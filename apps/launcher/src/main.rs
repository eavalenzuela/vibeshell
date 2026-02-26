fn main() {
    common::init_logging();
    tracing::info!(app = "launcher", "starting up");
    println!("launcher started");
}
