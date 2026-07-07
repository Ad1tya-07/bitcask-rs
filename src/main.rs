use storage_engine::{BitCask, SyncMode};   // use your real crate name (underscores)

fn main() -> std::io::Result<()> {
    let mut db = BitCask::open("data_file/data.db", SyncMode::EveryWrite)?;
    db.put("user_2", "Bob")?;
    db.put("user_3", "Key")?;
    db.put("user_4", "Mike")?;

    println!("{}", db.get("user_2"));
    println!("{}", db.get("user_3"));
    println!("{}", db.get("user_4"));
    Ok(())
}