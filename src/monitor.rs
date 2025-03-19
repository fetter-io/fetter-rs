use crate::scan_fs::ScanFS;
use crate::util::logger;
use crate::util::ResultDynError;
use std::path::PathBuf;
use std::{thread, time::Duration};
use std::sync::Arc;

fn monitor_scan(
    exe_paths: Arc<Vec<PathBuf>>, // Use Arc to safely share ownership
    force_usite: bool,
    log: bool,
) {

    if log {
        logger!(module_path!(), "Preparing call to from_exes().");
    }

    let sfs = ScanFS::from_exes(&exe_paths, force_usite, log).expect("from_exes() failed.");
    let json = serde_json::to_string(&sfs).expect("serde serialiation failed.");

    if log {
        logger!(module_path!(), "Got JSON: {:?} characters", json.len());
    }

}


pub(crate) fn monitor_scan_loop(
    exe_paths: &Vec<PathBuf>,
    force_usite: bool,
    period: u64,
    log: bool,
) {
    let exe_paths = Arc::new(exe_paths.clone());

    loop {
        let aep = Arc::clone(&exe_paths);

        thread::spawn(move || monitor_scan(aep, force_usite, log));
        if log {
            logger!(module_path!(), "Sleeping {:?}", period);
        }
        thread::sleep(Duration::from_secs(period));
    }

}

