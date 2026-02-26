fn main() {
    common::init_logging();
    tracing::info!(app = "notifd", "starting up");
    println!("notifd started");
}
