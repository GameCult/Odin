fn main() {
    if let Err(error) = idunn_daemon::provisioning::run(std::env::args().skip(1)) {
        eprintln!("idunn-provision: {error:#}");
        std::process::exit(1);
    }
}
