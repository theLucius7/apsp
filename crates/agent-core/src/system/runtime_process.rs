use anyhow::{Context, Result, ensure};
use apsp_adapter_sdk::RuntimeInstance;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

fn bounded_read(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((maximum + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= maximum,
        "process observation exceeds its byte budget"
    );
    Ok(bytes)
}

fn start_ticks(text: &[u8], pid: u32) -> Result<u64> {
    let text = std::str::from_utf8(text)?;
    let (prefix, suffix) = text.rsplit_once(") ").context("invalid process stat")?;
    ensure!(
        prefix
            .split_once(" (")
            .context("invalid process identity")?
            .0
            .parse::<u32>()?
            == pid,
        "process identity changed"
    );
    let fields: Vec<_> = suffix.split_whitespace().collect();
    ensure!(
        fields.len() >= 20 && matches!(fields[0], "R" | "S" | "D" | "I"),
        "process is no longer running"
    );
    let start: u64 = fields[19].parse()?;
    ensure!(start != 0, "invalid process start time");
    Ok(start)
}

fn belongs_to_group(bytes: &[u8], expected: &str) -> Result<()> {
    ensure!(
        expected.starts_with('/')
            && expected != "/"
            && expected.len() <= 4096
            && expected
                .split('/')
                .skip(1)
                .all(|part| !part.is_empty() && !matches!(part, "." | ".."))
            && !expected.contains(['\0', '\r', '\n']),
        "invalid controlled cgroup"
    );
    let text = std::str::from_utf8(bytes)?;
    let mut matched = false;
    for line in text.lines() {
        let mut fields = line.splitn(3, ':');
        let hierarchy = fields.next().context("missing cgroup hierarchy")?;
        let controllers = fields.next().context("missing cgroup controllers")?;
        let group = fields.next().context("missing cgroup path")?;
        let relevant = (hierarchy == "0" && controllers.is_empty())
            || controllers.split(',').any(|value| value == "name=systemd");
        if relevant
            && (group == expected
                || group
                    .strip_prefix(expected)
                    .is_some_and(|tail| tail.starts_with('/')))
        {
            matched = true;
        }
    }
    ensure!(
        matched,
        "runtime main process is outside its controlled cgroup"
    );
    Ok(())
}

fn config_path(bytes: &[u8]) -> Result<(PathBuf, PathBuf)> {
    ensure!(bytes.last() == Some(&0), "truncated runtime command line");
    let args: Vec<_> = bytes[..bytes.len() - 1]
        .split(|byte| *byte == 0)
        .map(std::str::from_utf8)
        .collect::<std::result::Result<_, _>>()?;
    ensure!(
        !args.is_empty() && args.len() <= 64 && args.iter().all(|arg| !arg.is_empty()),
        "invalid runtime command line"
    );
    let binary = PathBuf::from(args[0]);
    ensure!(
        binary.is_absolute(),
        "runtime executable argument is not absolute"
    );
    let mut selected = None;
    let mut index = 1;
    while index < args.len() {
        let value = if matches!(args[index], "-c" | "--config") {
            index += 1;
            Some(
                *args
                    .get(index)
                    .context("missing runtime configuration argument")?,
            )
        } else {
            ensure!(
                !args[index].starts_with("-C")
                    && !args[index].starts_with("-c")
                    && args[index] != "--config-directory"
                    && !args[index].starts_with("--config-directory="),
                "ambiguous or multiple configuration sources are unsupported"
            );
            args[index].strip_prefix("--config=")
        };
        if let Some(value) = value {
            let path = PathBuf::from(value);
            ensure!(
                selected.is_none() && path.is_absolute() && !value.contains(['\r', '\n']),
                "runtime must use one absolute configuration path"
            );
            selected = Some(path);
        }
        index += 1;
    }
    Ok((
        binary,
        selected.context("runtime configuration argument is unknown")?,
    ))
}

fn inspect_at(proc_root: &Path, pid: u32, group: &str) -> Result<RuntimeInstance> {
    ensure!(pid != 0, "runtime process has no PID");
    let process = proc_root.join(pid.to_string());
    let before_stat = bounded_read(&process.join("stat"), 4096)?;
    let start = start_ticks(&before_stat, pid)?;
    let cmdline = bounded_read(&process.join("cmdline"), 65536)?;
    let (argument_binary, config_path) = config_path(&cmdline)?;
    let membership = bounded_read(&process.join("cgroup"), 16384)?;
    belongs_to_group(&membership, group)?;
    let boot = bounded_read(&proc_root.join("sys/kernel/random/boot_id"), 128)?;
    let boot = std::str::from_utf8(&boot)?.trim();
    ensure!(
        uuid::Uuid::parse_str(boot).is_ok_and(|value| !value.is_nil()),
        "invalid kernel boot identity"
    );
    let binary = fs::read_link(process.join("exe"))?;
    ensure!(
        binary.is_absolute() && !binary.to_string_lossy().ends_with(" (deleted)"),
        "runtime executable is not an installed ordinary path"
    );
    let binary = fs::canonicalize(binary)?;
    ensure!(
        fs::metadata(&binary)?.is_file() && fs::canonicalize(argument_binary)? == binary,
        "runtime executable differs from its command line"
    );
    let resolved_config = fs::canonicalize(&config_path)?;
    ensure!(
        fs::metadata(&resolved_config)?.is_file(),
        "runtime configuration is not an ordinary file"
    );
    ensure!(
        start_ticks(&bounded_read(&process.join("stat"), 4096)?, pid)? == start
            && bounded_read(&process.join("cmdline"), 65536)? == cmdline
            && bounded_read(&process.join("cgroup"), 16384)? == membership
            && fs::canonicalize(fs::read_link(process.join("exe"))?)? == binary
            && fs::canonicalize(&config_path)? == resolved_config,
        "runtime process changed during inspection"
    );
    let instance_id = format!(
        "{:x}",
        Sha256::digest(
            format!("apsp-process-instance-v1\0{boot}\0{pid}\0{start}\0{group}").as_bytes()
        )
    );
    Ok(RuntimeInstance {
        instance_id,
        binary_path: binary,
        config_path,
    })
}

pub(super) async fn inspect(pid: u32, group: &str) -> Result<RuntimeInstance> {
    ensure!(
        cfg!(target_os = "linux"),
        "runtime process inspection requires Linux procfs"
    );
    let group = group.to_owned();
    tokio::task::spawn_blocking(move || inspect_at(Path::new("/proc"), pid, &group)).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn stat(pid: u32, start: u64, state: &str) -> String {
        let mut fields = vec![state.to_owned()];
        fields.extend((0..18).map(|_| "0".to_owned()));
        fields.push(start.to_string());
        format!("{pid} (runtime with ) spaces) {}\n", fields.join(" "))
    }

    #[test]
    fn rejects_dead_reused_and_malformed_process_identity() {
        assert_eq!(start_ticks(stat(123, 17, "S").as_bytes(), 123).unwrap(), 17);
        for text in [
            stat(122, 17, "S"),
            stat(123, 0, "S"),
            stat(123, 17, "Z"),
            "123 malformed".into(),
        ] {
            assert!(start_ticks(text.as_bytes(), 123).is_err());
        }
    }

    #[test]
    fn requires_control_group_boundary_and_single_absolute_config() {
        assert!(
            belongs_to_group(
                b"0::/system.slice/example.service\n",
                "/system.slice/example.service"
            )
            .is_ok()
        );
        assert!(
            belongs_to_group(
                b"0::/system.slice/example.service/helper\n",
                "/system.slice/example.service"
            )
            .is_ok()
        );
        assert!(
            belongs_to_group(
                b"1:name=systemd:/system.slice/example.service\n",
                "/system.slice/example.service"
            )
            .is_ok()
        );
        assert!(
            belongs_to_group(
                b"0::/system.slice/example.service-other\n",
                "/system.slice/example.service"
            )
            .is_err()
        );
        for value in [
            b"/bin/example\0run\0-c\0relative\0".as_slice(),
            b"/bin/example\0run\0-c\0/config\0-c\0/other\0",
            b"/bin/example\0run\0-c\0/config\0-C\0/other\0",
            b"/bin/example\0run\0-c\0/config",
            b"/bin/example\0run\0",
            b"/bin/example\0run\0-c\0/config\0-C/other\0",
            b"/bin/example\0run\0-c\0/config\0-C=/other\0",
            b"/bin/example\0run\0-c\0/config\0-c/other\0",
            b"/bin/example\0run\0-c\0/config\0-c=/other\0",
        ] {
            assert!(config_path(value).is_err());
        }
        assert_eq!(
            config_path(b"/bin/example\0run\0--config=/config\0")
                .unwrap()
                .1,
            Path::new("/config")
        );
    }

    #[test]
    fn stable_command_path_survives_identical_revision_link_switch() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "apsp-process-noop-TEST_ONLY-{}",
            uuid::Uuid::new_v4()
        ));
        let result: Result<()> = (|| {
            let process = root.join("proc/123");
            fs::create_dir_all(&process)?;
            fs::create_dir_all(root.join("proc/sys/kernel/random"))?;
            let binary = root.join("example-runtime");
            fs::write(&binary, b"TEST_ONLY inert bytes")?;
            for revision in ["1", "2"] {
                let directory = root.join("revisions").join(revision);
                fs::create_dir_all(&directory)?;
                fs::write(directory.join("config.json"), b"{}")?;
            }
            let current = root.join("current");
            symlink(root.join("revisions/1"), &current)?;
            let config = current.join("config.json");
            fs::write(process.join("stat"), stat(123, 42, "S"))?;
            fs::write(
                process.join("cmdline"),
                format!("{}\0run\0-c\0{}\0", binary.display(), config.display()),
            )?;
            fs::write(process.join("cgroup"), "0::/system.slice/example.service\n")?;
            fs::write(
                root.join("proc/sys/kernel/random/boot_id"),
                uuid::Uuid::new_v4().to_string(),
            )?;
            symlink(&binary, process.join("exe"))?;
            let first = inspect_at(&root.join("proc"), 123, "/system.slice/example.service")?;
            assert_eq!(first.config_path, config);
            fs::remove_file(&current)?;
            symlink(root.join("revisions/2"), &current)?;
            assert_eq!(
                first,
                inspect_at(&root.join("proc"), 123, "/system.slice/example.service")?
            );
            fs::remove_file(root.join("revisions/2/config.json"))?;
            assert!(inspect_at(&root.join("proc"), 123, "/system.slice/example.service").is_err());
            Ok(())
        })();
        let cleanup = fs::remove_dir_all(&root);
        result?;
        cleanup?;
        Ok(())
    }

    #[test]
    fn inspects_proc_bytes_and_rejects_wrong_instance_and_oversized_input() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("apsp-process-TEST_ONLY-{}", uuid::Uuid::new_v4()));
        let result: Result<()> = (|| {
            let process = root.join("proc/123");
            fs::create_dir_all(&process)?;
            fs::create_dir_all(root.join("proc/sys/kernel/random"))?;
            let binary = root.join("example-runtime");
            let config = root.join("config.json");
            fs::write(&binary, b"TEST_ONLY inert bytes")?;
            fs::write(&config, b"{}")?;
            fs::write(process.join("stat"), stat(123, 42, "S"))?;
            let command = format!("{}\0run\0-c\0{}\0", binary.display(), config.display());
            fs::write(process.join("cmdline"), command)?;
            fs::write(process.join("cgroup"), "0::/system.slice/example.service\n")?;
            fs::write(
                root.join("proc/sys/kernel/random/boot_id"),
                uuid::Uuid::new_v4().to_string(),
            )?;
            symlink(&binary, process.join("exe"))?;
            let first = inspect_at(&root.join("proc"), 123, "/system.slice/example.service")?;
            assert_eq!(first.binary_path, fs::canonicalize(&binary)?);
            assert_eq!(first.config_path, config);
            assert_eq!(
                first,
                inspect_at(&root.join("proc"), 123, "/system.slice/example.service")?
            );
            assert!(inspect_at(&root.join("proc"), 123, "/system.slice/other.service").is_err());
            fs::write(process.join("stat"), stat(123, 43, "S"))?;
            assert_ne!(
                first.instance_id,
                inspect_at(&root.join("proc"), 123, "/system.slice/example.service")?.instance_id
            );
            fs::write(process.join("cmdline"), vec![b'x'; 65537])?;
            assert!(inspect_at(&root.join("proc"), 123, "/system.slice/example.service").is_err());
            Ok(())
        })();
        let cleanup = fs::remove_dir_all(&root);
        result?;
        cleanup?;
        Ok(())
    }
}
