use super::*;
use std::collections::VecDeque;
use tokio_util::sync::CancellationToken;

pub(super) struct Fixture {
    directory: tempfile::TempDir,
    pub agent: Agent,
    pub store: SqliteStore,
    shutdown: CancellationToken,
    daemon: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl Fixture {
    pub async fn new() -> Self {
        let directory = tempfile::tempdir().expect("isolated fixture");
        let store = SqliteStore::open(directory.path().join("store.db")).unwrap();
        let sample = crate::testing::app_with_transcript(&[]);
        let mut agent = sample.selected_agent().unwrap().clone();
        agent.status = AgentStatus::Waiting {
            condition: WaitCondition::Inbox,
        };
        agent.paused = true;
        let mut fixture = Self {
            directory,
            agent,
            store,
            shutdown: CancellationToken::new(),
            daemon: None,
        };
        fixture.put_agent(fixture.agent.clone());
        fixture.start().await;
        assert!(fixture.daemon.is_some());
        assert_eq!(fixture.store.agents(&Default::default()).unwrap().len(), 1);
        fixture
    }

    pub fn put_agent(&self, agent: Agent) {
        assert_eq!(agent.run, self.agent.run);
        self.store
            .commit(Commit::new().mutate(Mutation::UpsertAgent(agent.clone())))
            .unwrap();
        assert_eq!(self.store.agent(agent.id).unwrap(), Some(agent));
    }

    pub fn commit(&self, kinds: Vec<EventKind>) -> Vec<aether_core::event::Event> {
        assert!(!kinds.is_empty());
        let count = kinds.len();
        let events = self
            .store
            .commit(Commit {
                events: kinds
                    .into_iter()
                    .map(|kind| {
                        EventDraft::new(Actor::System, kind)
                            .agent(self.agent.id)
                            .run(self.agent.run)
                    })
                    .collect(),
                mutations: vec![],
            })
            .unwrap();
        assert_eq!(events.len(), count);
        events
    }

    async fn start(&mut self) {
        assert!(self.daemon.is_none());
        self.shutdown = CancellationToken::new();
        self.daemon = Some(tokio::spawn(aether_daemon::run(
            aether_daemon::DaemonConfig {
                socket: self.directory.path().join("d.sock"),
                store_path: self.directory.path().join("store.db"),
                config_dir: Some(self.directory.path().join("config")),
                workspace_root: Some(self.directory.path().into()),
                max_concurrent_turns: Some(1),
                yolo: false,
                tcp: None,
                allow_remote: false,
            },
            self.shutdown.clone(),
        )));
        let client = self.client().await;
        assert!(client.welcome().is_some());
    }

    pub async fn client(&self) -> Client {
        super::super::history_tests::connect_when_ready(&self.directory.path().join("d.sock")).await
    }

    pub fn socket(&self) -> std::path::PathBuf {
        self.directory.path().join("d.sock")
    }

    pub async fn stop(&mut self) {
        assert!(self.daemon.is_some());
        self.shutdown.cancel();
        self.daemon.take().unwrap().await.unwrap().unwrap();
        assert!(self.daemon.is_none());
    }

    pub async fn restart(&mut self) {
        self.stop().await;
        self.start().await;
    }

    pub async fn fresh(&self) -> App {
        let mut app = App::new(TuiOptions {
            focus: Some(self.agent.id),
            auto_create_root: false,
            socket_path: Some(self.directory.path().join("d.sock")),
            ..Default::default()
        });
        self.reconnect(&mut app).await;
        assert!(app.connected);
        assert!(app.selected.is_some());
        app
    }

    pub async fn reconnect(&self, app: &mut App) -> Link {
        let client = self.reconnect_client(app).await;
        self.catch_up(app, Link::new(client)).await
    }

    /// Reconnect as the loop does when `previous` was the view's connection.
    pub async fn reconnect_after(&self, app: &mut App, previous: &Link) -> Link {
        let client = self.reconnect_client(app).await;
        self.catch_up(app, previous.reconnected(client)).await
    }

    async fn catch_up(&self, app: &mut App, link: Link) -> Link {
        let effects = app.reconnected(&link.client);
        let after = effects
            .iter()
            .find_map(|e| match e {
                Effect::Subscribe { after } => Some(*after),
                _ => None,
            })
            .expect("reconnect subscribes");
        let last = self.store.last_seq().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        spawn_subscription(link.clone(), tx, after);
        self.answer(app, &link.client, effects).await;
        loop {
            let next = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .unwrap()
                .unwrap();
            let LoopEvent::AppFrom {
                event,
                instance,
                generation,
            } = next
            else {
                panic!("scoped event");
            };
            assert_eq!(Some(&instance), app.options.instance_id.as_ref());
            let event = link
                .admit(&instance, generation, event)
                .expect("the current connection's own event");
            let effects = app.reduce(event);
            self.answer(app, &link.client, effects).await;
            if app.subscription.is_some() && app.last_seq.unwrap_or(0) >= last {
                break;
            }
        }
        assert!(app.connected);
        assert!(app.last_seq.unwrap_or(0) >= last);
        link
    }

    async fn reconnect_client(&self, app: &mut App) -> Client {
        if app.options.instance_id.is_none() {
            return self.client().await;
        }
        app.options.socket_path = Some(self.socket());
        let (tx, mut rx) = mpsc::unbounded_channel();
        spawn_reconnect(app, tx);
        let event = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let LoopEvent::Reconnected(client) = event else {
            panic!("reconnect worker must return a connected client");
        };
        assert!(client.is_connected());
        assert!(client.welcome().is_some());
        client
    }

    pub async fn answer(&self, app: &mut App, client: &Client, effects: Vec<Effect>) {
        let mut pending: VecDeque<_> = effects.into();
        let mut requests = 0;
        while let Some(effect) = pending.pop_front() {
            let Effect::Request { command, then } = effect else {
                continue;
            };
            requests += 1;
            assert!(requests < 100, "request loop");
            let result = client.request(command).await;
            assert!(result.is_ok(), "{then:?}: {result:?}");
            pending.extend(app.reduce(AppEvent::Reply { then, result }));
        }
    }

    pub async fn load_all(&self, app: &mut App) -> Vec<String> {
        let agent = self.agent.id;
        let client = self.client().await;
        let mut windows = vec![models(app, agent)];
        while let Some(effect) = app.older_history_request(agent) {
            self.answer(app, &client, vec![effect]).await;
            windows.push(models(app, agent));
            assert!(windows.len() < 10);
        }
        // Compare complete history independently of the bounded window's cut point.
        let messages = windows
            .into_iter()
            .rev()
            .flatten()
            .filter_map(|m| match m {
                CellModel::Assistant { text } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!messages.is_empty());
        assert!(
            messages.windows(2).all(|pair| pair[0] < pair[1]),
            "messages stay ordered without duplicates across pages"
        );
        app.latest_history_request(agent);
        messages
    }
}

pub(super) fn models(app: &App, agent: AgentId) -> Vec<CellModel> {
    app.transcripts[&agent]
        .all_cells()
        .map(|cell| cell.model.clone())
        .collect()
}
