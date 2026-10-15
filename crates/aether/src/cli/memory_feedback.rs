use aether_core::id::{InvocationId, MemoryFeedbackId, MemoryId, TaskId};
use aether_core::memory_feedback::{MemoryEvidence, MemoryFeedbackSpec, MemoryVerdict};
use clap::{Args, Subcommand};

#[derive(Debug, Subcommand)]
pub enum MemoryFeedbackCmd {
    /// Record evidence on a memory this task had in context.
    Record {
        #[command(flatten)]
        feedback: MemoryFeedbackArgs,
        #[arg(long)]
        json: bool,
    },
    /// Withdraw one feedback record, keeping its history.
    Retract {
        feedback: MemoryFeedbackId,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        json: bool,
    },
    /// List the newest 100 feedback records, including retractions.
    List {
        memory: MemoryId,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub struct MemoryFeedbackArgs {
    memory: MemoryId,
    #[arg(long)]
    task: TaskId,
    #[arg(long)]
    invocation: Option<InvocationId>,
    #[arg(long, value_parser = parse_verdict)]
    verdict: MemoryVerdict,
    #[arg(long, value_parser = parse_evidence)]
    evidence: MemoryEvidence,
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
                task: feedback.task,
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
        return Ok(());
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
    Ok(())
}
