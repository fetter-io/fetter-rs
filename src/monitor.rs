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

/// The text of a response body for a message: trimmed, and shortened when long.
fn response_text(body: &str) -> String {
    const MAX: usize = 500;
    let t = body.trim();
    match t.char_indices().nth(MAX) {
        Some((i, _)) => format!("{}...", &t[..i]),
        None => t.to_string(),
    }
}

/// Scan, and post the scan (or, if unchanged since the previous post, only the system) to `url`. An `Err` says why nothing was posted, or why the post was refused.
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
) -> Result<(), String> {
    logger!(log, module_path!(), "Calling from_origin_paths().");
    let mut sfs = match ScanFS::from_origin_paths(&origin_paths, &config, log) {
        Ok(sfs) => sfs,
        Err(e) => {
            logger!(log, module_path!(), "Scan failed, not posting: {}", e);
            return Err(format!("Scan failed, not posting: {e}"));
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
        return Err("Previous scan unavailable, not posting.".to_string());
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
            return Err(format!("{e}, not posting."));
        }
    };

    logger!(log, module_path!(), "Sending {:?} characters.", body.len());
    let response = client.post_status(&url, &body);
    logger!(log, module_path!(), "Got response: {:?}", response);
    match response {
        Ok((status, _)) if (200..300).contains(&status) => Ok(()),
        Ok((status, text)) => {
            // the server did not take this scan: post a full scan next time
            *sfs_prev = None;
            let text = response_text(&text);
            Err(if text.is_empty() {
                format!("Post to {url} refused (HTTP {status}).")
            } else {
                format!("Post to {url} refused (HTTP {status}): {text}")
            })
        }
        Err(e) => {
            *sfs_prev = None;
            Err(format!("Post to {url} failed: {e}"))
        }
    }
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

    // once: a failure is the command's error
    if period == 0 {
        return monitor_scan(
            eps,
            st,
            sfs_prev_mutex,
            client,
            url_arc,
            tenant_arc,
            config.clone(),
            log,
        )
        .map_err(Into::into);
    }

    let (tx, rx) = mpsc::channel();

    // spawn a single worker thread
    thread::spawn(move || {
        while let Ok((eps, st, sfs_prev_mutex, client, url, tenant, config, log)) =
            rx.recv()
        {
            // periodically: a failure is reported, and the next period tries again
            if let Err(e) =
                monitor_scan(eps, st, sfs_prev_mutex, client, url, tenant, config, log)
            {
                eprintln!("fetter monitor-scan: {e}");
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecosystem::Ecosystem;

    /// Answers every post with a fixed status and body, or fails to connect.
    struct StatusClient(Option<(u16, &'static str)>);

    impl UreqClient for StatusClient {
        fn post(&self, _url: &str, _body: &str) -> Result<String, ureq::Error> {
            unreachable!("monitor-scan uses post_status")
        }
        fn post_status(
            &self,
            _url: &str,
            _body: &str,
        ) -> Result<(u16, String), ureq::Error> {
            match self.0 {
                Some((status, body)) => Ok((status, body.to_string())),
                None => Err(ureq::Error::ConnectionFailed),
            }
        }
        fn get(&self, _url: &str) -> Result<String, ureq::Error> {
            Err(ureq::Error::StatusCode(404))
        }
    }

    fn post_once(
        origin: &str,
        response: Option<(u16, &'static str)>,
    ) -> Result<(), String> {
        let config = ScanConfig::new(false, false).with_ecosystems(&[Ecosystem::Python]);
        monitor_scan(
            Arc::new(vec![PathBuf::from(origin)]),
            Arc::new(SystemTag::from_system().unwrap()),
            Arc::new(Mutex::new(None)),
            Arc::new(StatusClient(response)),
            Arc::new("http://orb.test/monitor_scan".to_string()),
            Arc::new("tenant".to_string()),
            config,
            FlagLog(false),
        )
    }

    fn python() -> &'static str {
        crate::util::default_python_exe_names()[0]
    }

    #[test]
    fn test_monitor_scan_accepted() {
        assert_eq!(post_once(python(), Some((204, ""))), Ok(()));
    }

    #[test]
    fn test_monitor_scan_refused_with_reason() {
        let e = post_once(
            python(),
            Some((429, "Ping limit exceeded for tenant: tenant (limit: 4 in 24 hours, found: 4)\n")),
        )
        .unwrap_err();
        assert_eq!(
            e,
            "Post to http://orb.test/monitor_scan refused (HTTP 429): Ping limit exceeded for tenant: tenant (limit: 4 in 24 hours, found: 4)"
        );
        let e = post_once(python(), Some((500, ""))).unwrap_err();
        assert_eq!(
            e,
            "Post to http://orb.test/monitor_scan refused (HTTP 500)."
        );
    }

    #[test]
    fn test_monitor_scan_unreachable() {
        let e = post_once(python(), None).unwrap_err();
        assert!(
            e.starts_with("Post to http://orb.test/monitor_scan failed: "),
            "{e}"
        );
    }

    #[test]
    fn test_monitor_scan_scan_failed() {
        let e = post_once("a", Some((204, ""))).unwrap_err();
        assert!(e.starts_with("Scan failed, not posting: "), "{e}");
    }

    #[test]
    fn test_monitor_scan_loop_once_returns_the_failure() {
        let config = ScanConfig::new(false, false).with_ecosystems(&[Ecosystem::Python]);
        let r = monitor_scan_loop(
            &[PathBuf::from(python())],
            Arc::new(StatusClient(Some((400, "Tenant key not found: tenant")))),
            &"http://orb.test/monitor_scan".to_string(),
            &"tenant".to_string(),
            &config,
            0,
            FlagLog(false),
        );
        assert!(r
            .unwrap_err()
            .to_string()
            .ends_with("(HTTP 400): Tenant key not found: tenant"));
    }

    #[test]
    fn test_response_text() {
        assert_eq!(response_text("  ok \n"), "ok");
        let long = "x".repeat(600);
        assert_eq!(response_text(&long), format!("{}...", "x".repeat(500)));
    }
}
