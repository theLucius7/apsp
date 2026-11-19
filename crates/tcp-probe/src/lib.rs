#![forbid(unsafe_code)]

mod cli;
mod engine;
mod journal;
mod model;

pub use cli::{Command, Options, parse};
pub use engine::{run, run_until};
pub use journal::Journal;
pub use model::{IpVersion, Report, Snapshot, Target};

use anyhow::{Context, ensure};
use std::{ffi::OsString, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    time::{Instant, timeout_at},
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SOURCE_COMMIT: Option<&str> = option_env!("APSP_NATIVE_TCP_SOURCE_COMMIT");
const _: () = {
    if let Some(commit) = SOURCE_COMMIT {
        let bytes = commit.as_bytes();
        assert!(
            bytes.len() == 40,
            "source commit must have 40 hexadecimal digits"
        );
        let mut index = 0;
        while index < bytes.len() {
            assert!(
                matches!(bytes[index], b'0'..=b'9' | b'a'..=b'f'),
                "source commit must use lowercase hexadecimal digits"
            );
            index += 1;
        }
    }
};
pub const INPUT_LIMIT: usize = 16 * 1024;
pub const OUTPUT_LIMIT: usize = 64 * 1024;

pub fn run_cli<I, T>(args: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let mut raw_args = args.into_iter();
    let _program = raw_args.next();
    let mut utf8_args = Vec::new();
    for arg in raw_args {
        match arg.into().into_string() {
            Ok(value) => utf8_args.push(value),
            Err(_) => {
                eprintln!("TCP 诊断参数无效：argument is not valid UTF-8");
                std::process::exit(2);
            }
        }
    }
    let command = match parse(utf8_args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("TCP 诊断参数无效：{error}");
            std::process::exit(2);
        }
    };
    match command {
        Command::Help => {
            println!(
                "原生 TCP 连接诊断（无排名、上传或测速）\n用法：apsp-tcp-probe --workspace <绝对私有目录> --targets <目录内快照文件名> --target-digest <SHA256> --ip-version <4|6> --count <4|8> --concurrency <1|2> --no-rank-upload\n默认 count=4、concurrency=1；最多8目标，总60秒含排队，预留2秒保存。只建立并关闭TCP连接，不发送应用数据。地区/运营商为配置标签；结果不是包丢失率。--version 查看版本。\n不接受宿主改动、raw socket、测速或任意选项。"
            );
        }
        Command::Version => println!("apsp-tcp-probe {VERSION}"),
        Command::BuildInfo => println!(
            "{}",
            serde_json::json!({"version":VERSION,"source_repo":"theLucius7/apsp","source_commit":SOURCE_COMMIT})
        ),
        Command::Run(options) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .max_blocking_threads(4)
                .build()
                .expect("create probe runtime");
            let deadline = Instant::now() + Duration::from_secs(60);
            let result = runtime.block_on(async {
                timeout_at(deadline, async {
                    let mut journal = Journal::open(&options).await?;
                    // Keep bounded stdout publication inside the original process deadline.
                    let report =
                        run_until(&options, &mut journal, deadline - Duration::from_secs(2))
                            .await?;
                    let mut bytes = serde_json::to_vec(&report)?;
                    ensure!(bytes.len() <= OUTPUT_LIMIT, "report exceeds output limit");
                    bytes.push(b'\n');
                    let mut stdout = tokio::io::stdout();
                    timeout_at(
                        deadline.min(Instant::now() + Duration::from_secs(2)),
                        async {
                            stdout.write_all(&bytes).await?;
                            stdout.flush().await
                        },
                    )
                    .await
                    .context("stdout publication timed out")??;
                    Ok::<_, anyhow::Error>(report.complete)
                })
                .await
                .context("total execution deadline exceeded")
                .and_then(|result| result)
            });
            if let Err(error) = &result {
                let bytes = format!("TCP 诊断失败：{error}\n");
                runtime.block_on(async {
                    let mut stderr = tokio::io::stderr();
                    let _ = timeout_at(
                        deadline.min(Instant::now() + Duration::from_secs(2)),
                        async {
                            stderr.write_all(bytes.as_bytes()).await?;
                            stderr.flush().await
                        },
                    )
                    .await;
                });
            }
            // A timed-out system resolver must not delay process exit indefinitely.
            runtime.shutdown_timeout(Duration::from_millis(100));
            match result {
                Ok(complete) => {
                    if !complete {
                        std::process::exit(1);
                    }
                }
                Err(_) => std::process::exit(1),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
