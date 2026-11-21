fn main() {
    let payload = std::env::args().skip_while(|arg| arg != "--").nth(1);
    if let Some(payload) = payload {
        println!("{}", payload.chars().count());
    }
}
