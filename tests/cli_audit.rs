// use assert_cmd::Command;
// use std::fs;
// use std::path::PathBuf;
// use tempfile::tempdir;

// fn setup_fake_python_vulnerable() -> (tempfile::TempDir, PathBuf) {
//     let fake_python_dir = tempdir().unwrap();
//     let fake_python = fake_python_dir.path().join("python3");

//     let site_dir = tempdir().unwrap();
//     let site_packages = site_dir.path();

//     let dist_info = site_packages.join("requests-2.31.0.dist-info");
//     fs::create_dir(&dist_info).unwrap();

//     let metadata_path = dist_info.join("METADATA");
//     fs::write(
//         &metadata_path,
//         "\
//         Name: requests
//         Version: 2.31.0
//         ",
//     ).unwrap();

//     let script_contents = format!(
//         "#!/bin/bash
//         echo 'False'
//         echo '{}'
//         echo '{}'
//         ",
//         site_packages.to_str().unwrap(),
//         site_packages.to_str().unwrap()
//     );
//     fs::write(&fake_python, &script_contents).unwrap();

//     // On Unix, mark it executable
//     #[cfg(unix)]
//     {
//         use std::os::unix::fs::PermissionsExt;
//         let mut perms = fs::metadata(&fake_python).unwrap().permissions();
//         perms.set_mode(0o755);
//         fs::set_permissions(&fake_python, perms).unwrap();
//     }

//     (fake_python_dir, fake_python)
// }

// #[test]
// fn test_audit_exit_vulnerabilities_default_code() {
//     let (_py_dir, fake_python) = setup_fake_python_vulnerable();

//     let mut cmd = Command::cargo_bin("fetter").unwrap();
//     cmd.args(&[
//         "--exe", fake_python.to_str().unwrap(),
//         "audit",
//         "--pattern", "*",
//         "exit",
//     ]);
//     // Expects default exit code
//     cmd.assert().failure().code(3);
// }

// #[test]
// fn test_audit_exit_vulnerabilities_custom_code() {
//     let (_py_dir, fake_python) = setup_fake_python_vulnerable();

//     let mut cmd = Command::cargo_bin("fetter").unwrap();
//     cmd.args(&[
//         "--exe", fake_python.to_str().unwrap(),
//         "audit",
//         "--pattern", "*",
//         "exit",
//         "--code", "5",
//     ]);
//     // Expects exit code (user-inputted) 5
//     cmd.assert().failure().code(5);
// }
