//! Bulk import (stub).
use crate::app::State;
use crate::jobs::Job;

pub async fn import_job(_app: &State, _job: &mut Job) -> Result<(), String> {
    Ok(())
}
