//! Durable acquisition records. All timestamps are Unix milliseconds.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionSource {
    pub source_url: String,
    pub evidence: String,
    pub observed_at: i64,
}

#[derive(Debug)]
pub struct AcquisitionAnalysisBatch {
    pub sources: Vec<String>,
    pub(crate) lead_id: String,
    pub(crate) revision: i64,
    pub(crate) through: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AcquisitionOutreachPolicy {
    pub desktop_vision: bool,
    pub mode: String,
    pub message_template: String,
    pub max_messages_per_run: u32,
    pub max_messages_per_day: u32,
    pub min_interval_seconds: u32,
    pub min_intent_score: u8,
    pub approved_at: Option<i64>,
}

impl Default for AcquisitionOutreachPolicy {
    fn default() -> Self {
        Self {
            desktop_vision: false,
            mode: "draft".into(),
            message_template: String::new(),
            max_messages_per_run: 5,
            max_messages_per_day: 20,
            min_interval_seconds: 60,
            min_intent_score: 70,
            approved_at: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionCampaign {
    pub id: String,
    pub name: String,
    pub product: String,
    pub target_audience: String,
    pub keywords: Vec<String>,
    pub platforms: Vec<String>,
    pub max_leads: u32,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_error: Option<String>,
    #[serde(default)]
    pub outreach: AcquisitionOutreachPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionLead {
    pub id: String,
    pub campaign_id: String,
    pub platform: String,
    pub platform_user_id: String,
    pub display_name: String,
    pub profile_url: String,
    pub source_url: String,
    pub evidence: String,
    pub bio: String,
    pub status: String,
    pub intent_score: u8,
    pub tags: Vec<String>,
    pub profile: Value,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionMessage {
    pub id: String,
    pub lead_id: String,
    pub direction: String,
    pub body: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionRequirement {
    pub id: String,
    pub lead_id: String,
    pub summary: String,
    pub category: String,
    pub urgency: String,
    pub evidence: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcquisitionActivity {
    pub id: String,
    pub campaign_id: Option<String>,
    pub lead_id: Option<String>,
    pub kind: String,
    pub detail: String,
    pub created_at: i64,
}
