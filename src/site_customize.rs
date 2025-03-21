use crate::path_shared::PathShared;
use crate::util::logger;
use crate::validation_report::ValidationFlags;
use std::fs;
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

// fetter_launcher.pth
//      always called in start unless -S
//      can be invoked with site.main()
//      imports fetter_validate.py
// fetter_validate.py
//      import fetter and call fetter.run, in the same process

// Return a Vec of just the arguments for the validate command
fn get_validate_args(
    bound: &Path,
    bound_options: &Option<Vec<String>>,
    ignore: &[String],
    vf: &ValidationFlags,
) -> Vec<String> {
    let mut args = Vec::new();
    args.push("--bound".to_string());
    args.push(bound.display().to_string());
    if let Some(bo) = bound_options {
        args.push("--bound_options".to_string());
        args.extend(bo.iter().cloned());
    }
    if !ignore.is_empty() {
        args.push("--ignore".to_string());
        args.extend(ignore.iter().cloned());
    }
    if vf.permit_subset {
        args.push("--subset".to_string());
    }
    if vf.permit_superset {
        args.push("--superset".to_string());
    }
    args
}

fn get_validate_command(
    executable: &Path, // only accept one
    bound: &Path,
    bound_options: &Option<Vec<String>>,
    ignore: &[String],
    vf: &ValidationFlags,
) -> Vec<String> {
    let validate_args = get_validate_args(bound, bound_options, ignore, vf);
    let banner = format!("validate {}", validate_args.join(" "));

    let mut args = vec![
        "fetter".to_string(),
        "-b".to_string(),
        banner,
        "--cache-duration".to_string(),
        "0".to_string(),
        "--stderr".to_string(),
        "-e".to_string(),
        executable.display().to_string(),
        "validate".to_string(),
    ];
    args.extend(validate_args);
    args.push("display".to_string());
    args
}

fn get_validation_module(
    executable: &Path,
    bound: &Path,
    bound_options: &Option<Vec<String>>,
    ignore: &[String],
    vf: &ValidationFlags,
    exit_else_warn: Option<i32>,
    _cwd_option: Option<PathBuf>,
) -> String {
    let mut cmd_args = get_validate_command(executable, bound, bound_options, ignore, vf);

    let eew = exit_else_warn.map_or(Vec::with_capacity(0), |i| {
        vec!["--code".to_string(), format!("{}", i)]
    });
    cmd_args.extend(eew);

    // quote all arguments to represent as Python strings
    let cmd = format!(
        "[{}]",
        cmd_args
            .iter()
            .map(|v| format!("'{}'", v))
            .collect::<Vec<_>>()
            .join(", ")
    );
    // we exclude fetter and package managers from ever running; we match on startswith to catch "pip3" and related names.
    [
        "import sys",
        "import fetter",
        "from pathlib import Path",
        "run = True",
        "if sys.argv:",
        "    name = Path(sys.argv[0]).name",
        "    run = not any(name.startswith(n) for n in ('fetter', 'pip', 'poetry', 'uv'))",
        &format!("if run: fetter.run({})", cmd),
        "", // force a new line at end
    ].join("\n")
}

const FN_LAUNCHER_PTH: &str = "fetter_launcher.pth";
const FN_VALIDATE_PY: &str = "fetter_validate.py";

#[allow(clippy::too_many_arguments)]
pub(crate) fn install_validation(
    executable: &Path,
    bound: &Path,
    bound_options: &Option<Vec<String>>,
    ignore: &[String],
    vf: &ValidationFlags,
    exit_else_warn: Option<i32>,
    site: &PathShared,
    cwd_option: Option<PathBuf>,
    log: bool,
) -> io::Result<()> {
    let module_code = get_validation_module(
        executable,
        bound,
        bound_options,
        ignore,
        vf,
        exit_else_warn,
        cwd_option,
    );
    let fp_validate = site.join(FN_VALIDATE_PY);
    logger!(log, module_path!(), "Writing: {}", fp_validate.display());

    let mut file = File::create(&fp_validate)?;
    writeln!(file, "{}", module_code)?;

    let fp_launcher = site.join(FN_LAUNCHER_PTH);
    logger!(log, module_path!(), "Writing: {}", fp_launcher.display());

    let mut file = File::create(&fp_launcher)?;
    writeln!(file, "import fetter_validate\n")?;

    Ok(())
}

pub(crate) fn uninstall_validation(site: &PathShared, log: bool) -> io::Result<()> {
    let fp_launcher = site.join(FN_LAUNCHER_PTH);
    logger!(log, module_path!(), "Removing: {}", fp_launcher.display());

    let _ = fs::remove_file(fp_launcher);
    let fp_validate = site.join(FN_VALIDATE_PY);
    logger!(log, module_path!(), "Removing: {}", fp_validate.display());

    let _ = fs::remove_file(fp_validate);
    Ok(())
}

//------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_get_validation_command_a() {
        let exe = PathBuf::from("python3");
        let bound = PathBuf::from("requirements.txt");
        let bound_options = None;
        let vf = ValidationFlags {
            permit_superset: false,
            permit_subset: true,
        };
        let ignore = vec![];
        let post = get_validate_command(&exe, &bound, &bound_options, &ignore, &vf);
        assert_eq!(
            post,
            vec![
                "fetter",
                "-b",
                "validate --bound requirements.txt --subset",
                "--cache-duration",
                "0",
                "--stderr",
                "-e",
                "python3",
                "validate",
                "--bound",
                "requirements.txt",
                "--subset",
                "display",
            ]
        )
    }
    #[test]
    fn test_get_validation_command_b() {
        let exe = PathBuf::from("python3");
        let bound = PathBuf::from("requirements.txt");
        let bound_options = Some(vec!["foo".to_string(), "bar".to_string()]);
        let vf = ValidationFlags {
            permit_superset: true,
            permit_subset: true,
        };
        let ignore = vec![];
        let post = get_validate_command(&exe, &bound, &bound_options, &ignore, &vf);
        assert_eq!(post, vec!["fetter", "-b", "validate --bound requirements.txt --bound_options foo bar --subset --superset", "--cache-duration", "0", "--stderr", "-e", "python3", "validate", "--bound", "requirements.txt", "--bound_options", "foo", "bar", "--subset", "--superset", "display"])
    }
    #[test]
    fn test_get_validation_command_c() {
        let exe = PathBuf::from("python3");
        let bound = PathBuf::from("requirements.txt");
        let bound_options = Some(vec!["foo".to_string(), "bar".to_string()]);
        let vf = ValidationFlags {
            permit_superset: true,
            permit_subset: true,
        };
        let ignore = vec![];
        let post = get_validate_command(&exe, &bound, &bound_options, &ignore, &vf);
        assert_eq!(post, vec!["fetter", "-b", "validate --bound requirements.txt --bound_options foo bar --subset --superset", "--cache-duration", "0", "--stderr", "-e", "python3", "validate", "--bound", "requirements.txt", "--bound_options", "foo", "bar", "--subset", "--superset", "display"])
    }
    //--------------------------------------------------------------------------

    #[test]
    fn test_get_validation_module_a() {
        let exe = PathBuf::from("python3");
        let bound = PathBuf::from("requirements.txt");
        let bound_options = None;
        let vf = ValidationFlags {
            permit_superset: false,
            permit_subset: true,
        };
        let ec: Option<i32> = Some(4);
        let ignore = vec![];
        let post =
            get_validation_module(&exe, &bound, &bound_options, &ignore, &vf, ec, None);
        assert_eq!(post, "import sys\nimport fetter\nfrom pathlib import Path\nrun = True\nif sys.argv:\n    name = Path(sys.argv[0]).name\n    run = not any(name.startswith(n) for n in ('fetter', 'pip', 'poetry', 'uv'))\nif run: fetter.run(['fetter', '-b', 'validate --bound requirements.txt --subset', '--cache-duration', '0', '--stderr', '-e', 'python3', 'validate', '--bound', 'requirements.txt', '--subset', 'display', '--code', '4'])\n")
    }

    #[test]
    fn test_get_validation_module_b() {
        let exe = PathBuf::from("python3");
        let bound = PathBuf::from("requirements.txt");
        let bound_options = None;
        let vf = ValidationFlags {
            permit_superset: false,
            permit_subset: true,
        };
        let ec: Option<i32> = None;
        let ignore = vec![];
        let post =
            get_validation_module(&exe, &bound, &bound_options, &ignore, &vf, ec, None);
        assert_eq!(post, "import sys\nimport fetter\nfrom pathlib import Path\nrun = True\nif sys.argv:\n    name = Path(sys.argv[0]).name\n    run = not any(name.startswith(n) for n in ('fetter', 'pip', 'poetry', 'uv'))\nif run: fetter.run(['fetter', '-b', 'validate --bound requirements.txt --subset', '--cache-duration', '0', '--stderr', '-e', 'python3', 'validate', '--bound', 'requirements.txt', '--subset', 'display'])\n")
    }

    #[test]
    fn test_get_validation_module_c() {
        let exe = PathBuf::from("python3");
        let bound = PathBuf::from("requirements.txt");
        let bound_options = None;
        let vf = ValidationFlags {
            permit_superset: false,
            permit_subset: true,
        };
        let ec: Option<i32> = None;
        let cwd = Some(PathBuf::from("/home/foo"));
        let ignore = vec![];
        let post =
            get_validation_module(&exe, &bound, &bound_options, &ignore, &vf, ec, cwd);
        assert_eq!(post, "import sys\nimport fetter\nfrom pathlib import Path\nrun = True\nif sys.argv:\n    name = Path(sys.argv[0]).name\n    run = not any(name.startswith(n) for n in ('fetter', 'pip', 'poetry', 'uv'))\nif run: fetter.run(['fetter', '-b', 'validate --bound requirements.txt --subset', '--cache-duration', '0', '--stderr', '-e', 'python3', 'validate', '--bound', 'requirements.txt', '--subset', 'display'])\n")
    }

    #[test]
    fn test_get_validation_module_d() {
        let exe = PathBuf::from("python3");
        let bound = PathBuf::from("requirements.txt");
        let bound_options = None;
        let vf = ValidationFlags {
            permit_superset: false,
            permit_subset: true,
        };
        let ec: Option<i32> = None;
        let cwd = Some(PathBuf::from("/home/foo"));
        let ignore = vec!["pip".to_string()];
        let post =
            get_validation_module(&exe, &bound, &bound_options, &ignore, &vf, ec, cwd);
        assert_eq!(post, "import sys\nimport fetter\nfrom pathlib import Path\nrun = True\nif sys.argv:\n    name = Path(sys.argv[0]).name\n    run = not any(name.startswith(n) for n in ('fetter', 'pip', 'poetry', 'uv'))\nif run: fetter.run(['fetter', '-b', 'validate --bound requirements.txt --ignore pip --subset', '--cache-duration', '0', '--stderr', '-e', 'python3', 'validate', '--bound', 'requirements.txt', '--ignore', 'pip', '--subset', 'display'])\n")
    }
}
