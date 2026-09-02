fn main() -> anyhow::Result<()> {
    idunn_daemon::control_plane::run(std::env::args().skip(1))
}
