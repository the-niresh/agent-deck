use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

pub const APPROVAL_TIMEOUT_SECONDS: i64 = 36000; // 10 hours

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ApprovalRequest {
    pub id: String,
    pub tool_name: String,
    pub execution_process_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub timeout_at: DateTime<Utc>,
}

impl ApprovalRequest {
    pub fn new(tool_name: String, execution_process_id: Uuid) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            tool_name,
            execution_process_id,
            created_at: now,
            timeout_at: now + Duration::seconds(APPROVAL_TIMEOUT_SECONDS),
        }
    }
}

/// Status of a tool permission request (approve/deny for tool execution).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved {
        /// The user asked not to be prompted for this tool again.
        ///
        /// Each executor honours this in its own protocol: Claude echoes back
        /// the `permission_suggestions` the CLI offered, OpenCode replies
        /// "always" instead of "once". Executors with no such concept ignore
        /// it and simply approve once, which is the safe direction.
        #[serde(default)]
        always: bool,
    },
    Denied {
        #[ts(optional)]
        reason: Option<String>,
    },
    TimedOut,
}

/// A question–answer pair. `answer` holds one or more selected labels/values.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct QuestionAnswer {
    pub question: String,
    pub answer: Vec<String>,
}

/// Status of a question answer request.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QuestionStatus {
    Answered { answers: Vec<QuestionAnswer> },
    TimedOut,
}

// Tracks both approval and question answers requests
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ApprovalOutcome {
    Approved {
        /// See [`ApprovalStatus::Approved::always`]. Set by the UI's
        /// "Approve, don't ask again" action.
        #[serde(default)]
        always: bool,
    },
    Denied {
        #[ts(optional)]
        reason: Option<String>,
    },
    Answered {
        answers: Vec<QuestionAnswer>,
    },
    TimedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ApprovalResponse {
    pub execution_process_id: Uuid,
    pub status: ApprovalOutcome,
}
