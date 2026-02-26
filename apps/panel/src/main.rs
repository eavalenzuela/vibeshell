fn main() {
    common::init_logging();
    tracing::info!(app = "panel", "starting up");
    println!("panel started");
}
