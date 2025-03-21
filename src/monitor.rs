use crate::scan_fs::ScanFS;
use crate::system_tag::SystemTag;
use crate::util::logger;
use crate::util::ResultDynError;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use std::{thread, time::Duration};
use std::sync::Mutex;

fn monitor_scan(
    exe_paths: Arc<Vec<PathBuf>>,
    system_tag: Arc<SystemTag>,
    sfs_previous_option: Arc<Mutex<Option<ScanFS>>>,
    force_usite: bool,
    log: bool,
) {
    if log {
        logger!(module_path!(), "Calling from_exes().");
    }

    let mut sfs_previous = sfs_previous_option.lock().unwrap();

    let sfs =
        ScanFS::from_exes(&exe_paths, force_usite, log).expect("from_exes() failed.");

    if sfs_previous.as_ref() == Some(&sfs) {
        if log {
            logger!(module_path!(), "No change in scan results.");
        }
    } else {
        *sfs_previous = Some(sfs); // move into Arc<Mutex<Option<ScanFS>>>
        let sfs_ref = sfs_previous.as_ref().unwrap();

        let duration_since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");
        let data = (&*system_tag, sfs_ref, &duration_since_epoch);
        let json = serde_json::to_string(&data).expect("serialiation failed.");

        if log {
            logger!(
                module_path!(),
                "Generated JSON: {:?} characters",
                json.len()
            );
        }
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
    let sfs_previous: Arc<Mutex<Option<ScanFS>>> = Arc::new(Mutex::new(None));

    let (tx, rx) = mpsc::channel();

    // spawn a single worker thread
    thread::spawn(move || {
        while let Ok((eps, st, sfs_previous, force_usite, log)) = rx.recv() {
            monitor_scan(eps, st, sfs_previous, force_usite, log);
        }
    });

    loop {
        let eps_move = eps.clone();
        let st_move = st.clone();
        let sfsp_move = sfs_previous.clone();

        if let Err(e) = tx.send((eps_move, st_move, sfsp_move, force_usite, log)) {
            if log {
                logger!(module_path!(), "Worker panicked: {}", e);
            }
            return Err(format!("Failed to queue scan: {}", e).into());
        } else {
            // Ok
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
