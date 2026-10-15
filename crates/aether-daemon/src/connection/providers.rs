use crate::Daemon;
use aether_protocol::{AuthInfo, ErrorReply, ModelInfo, ProviderInfo, Reply};
use std::collections::HashMap;

use super::map_err;

pub(super) async fn list_models(daemon: &Daemon) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    if rt.default_model().is_none() {
        rt.set_default_model(daemon.models.default_model().await);
    }
    let default = rt
        .default_model()
        .map(|model| daemon.models.canonical(&model));
    let mut out = Vec::new();
    let mut auth_cache: HashMap<String, (bool, Option<String>)> = HashMap::new();
    for info in daemon.models.list() {
        let provider = info.model.provider.clone();
        if !auth_cache.contains_key(&provider) {
            let status = daemon.models.auth_status(&provider).await;
            auth_cache.insert(provider.clone(), (status.configured, status.source));
        }
        let (configured, source) = auth_cache.get(&provider).cloned().unwrap_or((false, None));
        out.push(ModelInfo {
            tier: info.tier.map(|tier| tier.as_str().to_owned()),
            is_default: default.as_ref() == Some(&info.model),
            model: info.model,
            aliases: info.aliases,
            name: info.name,
            provider_name: info.provider_name,
            capabilities: info.capabilities,
            configured,
            auth_source: source,
        });
    }
    Ok(Reply::Models(out))
}

pub(super) async fn auth_status(daemon: &Daemon) -> Result<Reply, ErrorReply> {
    let mut out = Vec::new();
    for provider in daemon
        .models
        .registry()
        .providers()
        .iter()
        .filter(|p| p.enabled)
    {
        let status = daemon.models.auth_status(&provider.id).await;
        let mut methods = Vec::new();
        if !provider.auth.api_key_env.is_empty() {
            methods.push("api_key".to_owned());
        }
        if provider.auth.oauth.is_some() {
            methods.push("oauth".to_owned());
        }
        if provider.auth.helper.is_some() {
            methods.push("helper".to_owned());
        }
        if provider.auth.import.is_some() {
            methods.push("import".to_owned());
        }
        if provider.auth.none {
            methods.push("none".to_owned());
        }
        let subscription = daemon.models.oauth_label(&provider.id).is_some_and(|l| {
            l.to_lowercase().contains("subscription") || l.to_lowercase().contains("chatgpt")
        });
        let (detail, program) = match &provider.auth.cli {
            Some(cli) if !status.configured => {
                (Some(cli.status().await.detail), Some(cli.program.clone()))
            }
            Some(cli) => (None, Some(cli.program.clone())),
            None => (status.error.clone(), None),
        };
        out.push(AuthInfo {
            provider: provider.id.clone(),
            provider_name: provider.name.clone(),
            configured: status.configured,
            source: status.source,
            methods,
            subscription,
            detail,
            program,
        });
    }
    Ok(Reply::Auth(out))
}

pub(super) async fn providers(daemon: &Daemon) -> Result<Reply, ErrorReply> {
    let rt = &daemon.runtime;
    let mut out = Vec::new();
    // Disabled providers are listed too: seeing one turned off is the
    // answer to "why does this model not resolve".
    for provider in daemon.models.registry().providers() {
        // A disabled provider is unreachable, so probing its CLI or
        // key would only cost time and report a misleading "ready".
        let (configured, auth_source) = if provider.enabled {
            let status = daemon.models.auth_status(&provider.id).await;
            (status.configured, status.source)
        } else {
            (false, None)
        };
        out.push(ProviderInfo {
            provider: provider.id.clone(),
            name: provider.name.clone(),
            protocol: provider.protocol.as_str().to_owned(),
            family: provider.family.map(|f| f.as_str().to_owned()),
            base_url: provider.base_url.clone(),
            engine: provider.protocol.is_engine(),
            models: provider.models.len() as u32,
            open_model_list: provider.flags.open_model_list,
            enabled: provider.enabled,
            configured,
            auth_source,
            health: rt.provider_health_for(&provider.id).map_err(map_err)?,
        });
    }
    Ok(Reply::Providers(out))
}
