use crate::scan_fs::ScanFS;
use crate::system_tag::SystemTag;
use crate::ureq_client::UreqClient;
use crate::util::logger;
use crate::util::ResultDynError;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::{mpsc, Arc};
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use std::{thread, time::Duration};

fn monitor_scan(
    exe_paths: Arc<Vec<PathBuf>>,
    system_tag: Arc<SystemTag>,
    sfs_prev_mutex: Arc<Mutex<Option<ScanFS>>>,
    _client: Arc<dyn UreqClient>,
    force_usite: bool,
    log: bool,
) {
    logger!(log, module_path!(), "Calling from_exes().");
    let sfs =
        ScanFS::from_exes(&exe_paths, force_usite, log).expect("from_exes() failed.");

    let mut sfs_prev = sfs_prev_mutex.lock().unwrap();
    if sfs_prev.as_ref() == Some(&sfs) {
        logger!(log, module_path!(), "No change in scan results.");
    } else {
        *sfs_prev = Some(sfs); // move into Arc<Mutex<Option<ScanFS>>>
        let sfs_ref = sfs_prev.as_ref().expect("Could not get ref from mutex");

        let duration_since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");
        let data = (&*system_tag, sfs_ref, &duration_since_epoch);
        let json = serde_json::to_string(&data).expect("serialiation failed.");

        logger!(
            log,
            module_path!(),
            "Generated JSON: {:?} characters",
            json.len()
        );
    }
}

pub(crate) fn monitor_scan_loop(
    exe_paths: &[PathBuf],
    client: Arc<dyn UreqClient>,
    force_usite: bool,
    period: u64,
    log: bool,
) -> ResultDynError<()> {
    let eps = Arc::new(exe_paths.to_owned());
    let st = Arc::new(SystemTag::from_system().expect("failed from_system()"));
    // we hold the owned previous ScanFS
    let sfs_prev_mutex: Arc<Mutex<Option<ScanFS>>> = Arc::new(Mutex::new(None));

    let (tx, rx) = mpsc::channel();

    // spawn a single worker thread
    thread::spawn(move || {
        while let Ok((eps, st, sfs_prev_mutex, client, force_usite, log)) = rx.recv() {
            monitor_scan(eps, st, sfs_prev_mutex, client, force_usite, log);
        }
    });

    loop {
        if let Err(e) = tx.send((
            Arc::clone(&eps),
            Arc::clone(&st),
            Arc::clone(&sfs_prev_mutex),
            Arc::clone(&client),
            force_usite,
            log,
        )) {
            logger!(log, module_path!(), "Worker panicked: {}", e);
            return Err(format!("Failed to queue scan: {}", e).into());
        } else {
            // Ok
            logger!(log, module_path!(), "Queued a new scan.");
        }

        logger!(log, module_path!(), "Sleeping {:?}", period);
        thread::sleep(Duration::from_secs(period));
    }
}
