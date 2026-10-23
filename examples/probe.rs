#[path = "../src/game.rs"]
mod game;

fn main() {
    match game::GameSession::connect().and_then(|mut session| session.poll()) {
        Ok(state) => println!("{state:?}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
