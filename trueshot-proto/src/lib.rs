// Minimal protobuf struct for Job definition
// Ideally we run `protoc` but for now we define structs that mimic it.

use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ProcessingJob {
    pub id: String,
    pub input_s3_url: String,
    pub output_s3_url: String,
    pub params_json: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JobStatus {
    pub job_id: String,
    pub status: String, // "Running", "Completed", "Failed"
    pub progress: f32,
}
