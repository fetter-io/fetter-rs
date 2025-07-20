use crate::scan_fs::ScanFS;
use crate::system_tag::SystemTag;
use crate::ureq_client::UreqClient;
use crate::util::logger;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::{mpsc, Arc};
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use std::{thread, time::Duration};

#[allow(clippy::too_many_arguments)]
fn monitor_scan(
    exe_paths: Arc<Vec<PathBuf>>,
    system_tag: Arc<SystemTag>,
    sfs_prev_mutex: Arc<Mutex<Option<ScanFS>>>,
    client: Arc<dyn UreqClient>,
    url: Arc<String>,
    tenant: Arc<String>,
    force_usite: bool,
    log: FlagLog,
) {
    logger!(log, module_path!(), "Calling from_exes().");
    let sfs =
        ScanFS::from_exes(&exe_paths, force_usite, log).expect("from_exes() failed.");

    let duration_since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");

    let mut sfs_prev = sfs_prev_mutex.lock().unwrap();
    let data;

    if sfs_prev.as_ref() == Some(&sfs) {
        logger!(log, module_path!(), "Scan results unchanged.");
        data = (&*tenant, &*system_tag, None, &duration_since_epoch);
    } else {
        logger!(log, module_path!(), "Scan results new.");
        *sfs_prev = Some(sfs);
        let sfs_ref = sfs_prev.as_ref().expect("Could not get ref from mutex");
        data = (&*tenant, &*system_tag, Some(sfs_ref), &duration_since_epoch);
    }

    let body = serde_json::to_string(&data).expect("serialization failed.");

    logger!(log, module_path!(), "Sending {:?} characters.", body.len());
    // println!("{}", body);

    let response: Result<String, ureq::Error> = client.post(&url, &body);
    logger!(log, module_path!(), "Got response: {:?}", response);
}

pub(crate) fn monitor_scan_loop(
    exe_paths: &[PathBuf],
    client: Arc<dyn UreqClient>,
    url: &String,
    tenant: &String,
    force_usite: bool,
    period: u64,
    log: FlagLog,
) -> ResultDynError<()> {
    let eps = Arc::new(exe_paths.to_owned());
    let url_arc = Arc::new(url.to_owned());
    let tenant_arc = Arc::new(tenant.to_owned());

    let st = Arc::new(SystemTag::from_system().expect("failed from_system()"));
    // we hold the owned previous ScanFS
    let sfs_prev_mutex: Arc<Mutex<Option<ScanFS>>> = Arc::new(Mutex::new(None));

    let (tx, rx) = mpsc::channel();

    // spawn a single worker thread
    thread::spawn(move || {
        while let Ok((eps, st, sfs_prev_mutex, client, url, tenant, force_usite, log)) =
            rx.recv()
        {
            monitor_scan(
                eps,
                st,
                sfs_prev_mutex,
                client,
                url,
                tenant,
                force_usite,
                log,
            );
        }
    });

    loop {
        if let Err(e) = tx.send((
            Arc::clone(&eps),
            Arc::clone(&st),
            Arc::clone(&sfs_prev_mutex),
            Arc::clone(&client),
            Arc::clone(&url_arc),
            Arc::clone(&tenant_arc),
            force_usite,
            log,
        )) {
            logger!(log, module_path!(), "Worker panicked: {e}");
            return Err(format!("Failed to queue scan: {e}").into());
        } else {
            // Ok
            logger!(log, module_path!(), "Queued a new scan.");
        }

        logger!(log, module_path!(), "Sleeping {:?}", period);
        thread::sleep(Duration::from_secs(period));
    }
}
