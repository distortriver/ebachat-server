//! Bulk import (`import_process` jobs): the checks Go's worker makes on the job and its file. Importing the
//! JSONL lines themselves (`BulkImportWithPath`, app/import_functions.go) is not ported: the job fails with an
//! error instead of claiming a success it did not earn. See docs/JOBS.md.

// Based on Mattermost server/channels/jobs/import_process/worker.go, Copyright (c) 2015-present Mattermost, Inc.
// Licensed under AGPL-3.0.

use crate::app::State;
use crate::jobs::{self, Job};

pub async fn import_job(app: &State, job: &mut Job) -> Result<(), String> {
    let Some(file) = job.get("import_file").map(str::to_string) else {
        return Err(jobs::go_error("ImportProcessWorker", "import_process.worker.do_job.missing_file", ""));
    };
    if job.get("local_mode") == Some("true") {
        if !std::path::Path::new(&file).exists() {
            return Err(format!("file {file} doesn't exist."));
        }
    } else {
        let path = crate::files::clean(&format!("{}/{file}", app.cfg.str("ImportSettings.Directory")));
        if !app.files.exists(&path).await {
            return Err(jobs::go_error("ImportProcessWorker", "import_process.worker.do_job.file_exists", ""));
        }
    }
    Err(jobs::go_error("ImportProcessWorker", "api.not_implemented.app_error", "bulk import is not ported"))
}
