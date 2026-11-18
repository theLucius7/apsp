#![forbid(unsafe_code)]

fn main() -> anyhow::Result<()> {
    apsp_tcp_probe::run_cli(std::env::args_os())
}
