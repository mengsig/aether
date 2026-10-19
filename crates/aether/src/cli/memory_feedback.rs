use aether_core::id::{InvocationId, MemoryFeedbackId, MemoryId};
use aether_core::memory_feedback::{
    FeedbackRanking, MemoryEvidence, MemoryFeedbackSpec, MemoryVerdict, RANKING_MIN_VOTES,
    RANKING_WINDOW,
};
use clap::{Args, Subcommand};

#[derive(Debug, Subcommand)]
pub enum MemoryFeedbackCmd {
    /// Record evidence on a memory this task had in context.
    Record {
        #[command(flatten)]
        feedback: MemoryFeedbackArgs,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Withdraw one feedback record, keeping its history.
    Retract {
        feedback: MemoryFeedbackId,
        /// Why it is withdrawn.
        #[arg(long)]
        reason: String,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List the newest 100 feedback records, including retractions.
    List {
        memory: MemoryId,
        /// Print the result as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub struct MemoryFeedbackArgs {
    memory: MemoryId,
    /// The task that had the memory in context.
    #[arg(long)]
    task: super::TaskRef,
    /// The model call the evidence concerns.
    #[arg(long)]
    invocation: Option<InvocationId>,
    /// helped, hurt or neutral.
    #[arg(long, value_parser = parse_verdict)]
    verdict: MemoryVerdict,
    /// Where it comes from: operator, review or outcome.
    #[arg(long, value_parser = parse_evidence)]
    evidence: MemoryEvidence,
    /// What happened, in a sentence.
    #[arg(long)]
    reason: String,
}

fn parse_verdict(text: &str) -> anyhow::Result<MemoryVerdict> {
    serde_json::from_value(serde_json::Value::String(text.into()))
        .map_err(|_| anyhow::anyhow!("expected helped, hurt or neutral"))
}

fn parse_evidence(text: &str) -> anyhow::Result<MemoryEvidence> {
    serde_json::from_value(serde_json::Value::String(text.into()))
        .map_err(|_| anyhow::anyhow!("expected operator, review or outcome"))
}

pub async fn memory_feedback(
    socket: aether_client::Endpoint,
    command: MemoryFeedbackCmd,
) -> anyhow::Result<()> {
    let client = super::connect(&socket).await?;
    match command {
        MemoryFeedbackCmd::Record { feedback, json } => {
            let spec = MemoryFeedbackSpec {
                memory: feedback.memory,
                task: feedback.task.resolve(&client).await?,
                invocation: feedback.invocation,
                verdict: feedback.verdict,
                evidence: feedback.evidence,
                reason: feedback.reason,
            };
            let record = client.record_memory_feedback(spec).await?;
            if json {
                println!("{}", serde_json::to_string(&record)?);
            } else {
                println!("{} recorded for {}", record.id, record.spec.memory);
            }
        }
        MemoryFeedbackCmd::Retract {
            feedback,
            reason,
            json,
        } => {
            let record = client.retract_memory_feedback(feedback, reason).await?;
            if json {
                println!("{}", serde_json::to_string(&record)?);
            } else {
                println!("{} retracted; the record stays in history", record.id);
            }
        }
        MemoryFeedbackCmd::List { memory, json } => list(&client, memory, json).await?,
    }
    Ok(())
}

async fn list(client: &aether_client::Client, memory: MemoryId, json: bool) -> anyhow::Result<()> {
    let found = client.memory_feedback(memory, None).await?;
    anyhow::ensure!(
        found.records.len() <= 100,
        "daemon exceeded the feedback limit"
    );
    anyhow::ensure!(
        found.records.iter().all(|r| r.spec.memory == memory),
        "daemon returned feedback for another memory"
    );
    if json {
        println!("{}", serde_json::to_string(&found)?);
        return Ok(());
    }
    if found.records.is_empty() {
        println!("no feedback for {memory}.");
    }
    for record in found.records {
        println!(
            "{}  {}  {}  {}{}  {}",
            record.id,
            serde_json::to_value(record.spec.verdict)?,
            serde_json::to_value(record.spec.evidence)?,
            record.spec.task,
            if record.retracted_at.is_some() {
                " (retracted)"
            } else {
                ""
            },
            super::sane(&record.spec.reason)
        );
    }
    if found.truncated {
        println!("(only the newest 100 feedback records are listed)");
    }
    println!("{}", ranking_line(&found.ranking)?);
    Ok(())
}

/// "ranking factor 1.12 (3 helped, 1 hurt across the latest 20 tasks)".
fn ranking_line(ranking: &FeedbackRanking) -> anyhow::Result<String> {
    let votes = ranking.helped.saturating_add(ranking.hurt);
    anyhow::ensure!(
        votes as usize <= RANKING_WINDOW,
        "daemon counted more than {RANKING_WINDOW} tasks"
    );
    anyhow::ensure!(
        (0.75..=1.25).contains(&ranking.factor),
        "daemon reported a ranking factor outside 0.75 to 1.25"
    );
    let inactive = if votes < RANKING_MIN_VOTES {
        format!("; {RANKING_MIN_VOTES} needed to move recall")
    } else {
        String::new()
    };
    Ok(format!(
        "ranking factor {:.2} ({} helped, {} hurt across the latest {RANKING_WINDOW} tasks{inactive})",
        ranking.factor, ranking.helped, ranking.hurt
    ))
}
