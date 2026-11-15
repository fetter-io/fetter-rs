use crate::scan_fs::ScanFS;
use crate::system_tag::SystemTag;
use crate::transmit::payload_json;
use crate::ureq_client::UreqClient;
use crate::util::logger;
use crate::util::FlagLog;
use crate::util::ResultDynError;
use crate::util::ScanConfig;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::{mpsc, Arc};
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use std::{thread, time::Duration};

#[allow(clippy::too_many_arguments)]
fn monitor_scan(
    origin_paths: Arc<Vec<PathBuf>>,
    system_tag: Arc<SystemTag>,
    sfs_prev_mutex: Arc<Mutex<Option<ScanFS>>>,
    client: Arc<dyn UreqClient>,
    url: Arc<String>,
    tenant: Arc<String>,
    config: ScanConfig,
    log: FlagLog,
) {
    logger!(log, module_path!(), "Calling from_origin_paths().");
    // a failed scan or serialization skips this period; the worker continues
    let mut sfs = match ScanFS::from_origin_paths(&origin_paths, &config, log) {
        Ok(sfs) => sfs,
        Err(e) => {
            logger!(log, module_path!(), "Scan failed, not posting: {}", e);
            return;
        }
    };
    sfs.load_hooks();

    let duration_since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    let Ok(mut sfs_prev) = sfs_prev_mutex.lock() else {
        logger!(
            log,
            module_path!(),
            "Previous scan unavailable, not posting."
        );
        return;
    };

    let scan = if sfs_prev.as_ref() == Some(&sfs) {
        logger!(log, module_path!(), "Scan results unchanged.");
        None
    } else {
        logger!(log, module_path!(), "Scan results new.");
        *sfs_prev = Some(sfs);
        sfs_prev.as_ref()
    };

    let body = match payload_json(&tenant, &system_tag, &duration_since_epoch, scan) {
        Ok(b) => b,
        Err(e) => {
            logger!(log, module_path!(), "{}, not posting.", e);
            // post a full scan next time
            *sfs_prev = None;
            return;
        }
    };

    logger!(log, module_path!(), "Sending {:?} characters.", body.len());

    let response: Result<String, ureq::Error> = client.post(&url, &body);
    logger!(log, module_path!(), "Got response: {:?}", response);
}

pub(crate) fn monitor_scan_loop(
    origin_paths: &[PathBuf],
    client: Arc<dyn UreqClient>,
    url: &String,
    tenant: &String,
    config: &ScanConfig,
    period: u64,
    log: FlagLog,
) -> ResultDynError<()> {
    let eps = Arc::new(origin_paths.to_owned());
    let url_arc = Arc::new(url.to_owned());
    let tenant_arc = Arc::new(tenant.to_owned());

    let st = Arc::new(
        SystemTag::from_system()
            .map_err(|e| format!("Cannot read system details: {e}"))?,
    );
    // we hold the owned previous ScanFS
    let sfs_prev_mutex: Arc<Mutex<Option<ScanFS>>> = Arc::new(Mutex::new(None));

    if period == 0 {
        monitor_scan(
            eps,
            st,
            sfs_prev_mutex,
            client,
            url_arc,
            tenant_arc,
            config.clone(),
            log,
        );
        return Ok(());
    }

    let (tx, rx) = mpsc::channel();

    // spawn a single worker thread
    thread::spawn(move || {
        while let Ok((eps, st, sfs_prev_mutex, client, url, tenant, config, log)) =
            rx.recv()
        {
            monitor_scan(eps, st, sfs_prev_mutex, client, url, tenant, config, log);
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
            config.clone(),
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
