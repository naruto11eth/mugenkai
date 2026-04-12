fn main() -> std::process::ExitCode {
    eprintln!("mugenkai {}", host_core::VERSION);
    host_ui::run()
}
