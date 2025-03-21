use crate::scan_fs::ScanFS;
use crate::system_tag::SystemTag;
use crate::util::logger;
use crate::util::ResultDynError;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use std::{thread, time::Duration};

fn monitor_scan(
    exe_paths: Arc<Vec<PathBuf>>,
    system_tag: Arc<SystemTag>,
    force_usite: bool,
    log: bool,
) {
    if log {
        logger!(module_path!(), "Calling from_exes().");
    }

    let sfs =
        ScanFS::from_exes(&exe_paths, force_usite, log).expect("from_exes() failed.");

    let duration_since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    let data = (&*system_tag, &sfs, &duration_since_epoch);
    let json = serde_json::to_string(&data).expect("serialiation failed.");

    if log {
        logger!(
            module_path!(),
            "Generated JSON: {:?} characters",
            json.len()
        );
    }
}

pub(crate) fn monitor_scan_loop(
    exe_paths: &Vec<PathBuf>,
    force_usite: bool,
    period: u64,
    log: bool,
) -> ResultDynError<()> {
    let eps = Arc::new(exe_paths.clone());
    let st = Arc::new(SystemTag::from_system().expect("failed from_system()"));
    let (tx, rx) = mpsc::channel();

    // spawn a single worker thread
    thread::spawn(move || {
        while let Ok((eps, st, force_usite, log)) = rx.recv() {
            monitor_scan(eps, st, force_usite, log);
        }
    });

    loop {
        let eps_move = eps.clone();
        let st_move = st.clone();

        if let Err(e) = tx.send((eps_move, st_move, force_usite, log)) {
            if log {
                logger!(module_path!(), "Worker panicked: {}", e);
            }
            return Err(format!("Failed to queue scan: {}", e).into());
        } else {
            if log {
                logger!(module_path!(), "Queued a new scan.");
            }
        }

        if log {
            logger!(module_path!(), "Sleeping {:?}", period);
        }
        thread::sleep(Duration::from_secs(period));
    }
}
