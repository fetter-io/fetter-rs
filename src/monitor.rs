use crate::scan_fs::ScanFS;
use crate::system_tag::SystemTag;
use crate::util::logger;
use crate::util::ResultDynError;
use std::path::PathBuf;
use std::sync::Arc;
use std::{thread, time::Duration};

// TODO: can I reuse the same threads?

fn monitor_scan(
    exe_paths: Arc<Vec<PathBuf>>,
    system_tag: Arc<SystemTag>,
    force_usite: bool,
    log: bool,
) {
    if log {
        logger!(module_path!(), "Preparing call to from_exes().");
    }

    let sfs =
        ScanFS::from_exes(&exe_paths, force_usite, log).expect("from_exes() failed.");

    // TODO: add a timestamp
    let data = (&*system_tag, &sfs);
    let json = serde_json::to_string(&data).expect("serialiation failed.");

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
    let eps = Arc::new(exe_paths.clone());
    let st = Arc::new(SystemTag::from_system().expect("failed from_system()"));

    loop {
        let eps_move = eps.clone();
        let st_move = st.clone();
        thread::spawn(move || monitor_scan(eps_move, st_move, force_usite, log));
        if log {
            logger!(module_path!(), "Sleeping {:?}", period);
        }
        thread::sleep(Duration::from_secs(period));
    }
}
