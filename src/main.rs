use std::io::{Read, Write, Seek, SeekFrom, ErrorKind};
use std::fs;
use std::fs::{OpenOptions, File};
use std::collections::HashMap;
use std::path::PathBuf;

fn put(file: &mut File, key: &str, val: &str) -> std::io::Result<()>{
    file.seek(SeekFrom::End(0))?;
    let key_len = key.len() as u32;
    let val_len = val.len() as u32;
    file.write_all(&key_len.to_le_bytes())?;
    file.write_all(key.as_bytes())?;
    file.write_all(&val_len.to_le_bytes())?;
    file.write_all(val.as_bytes())?;
    Ok(())
}
fn get(key: &str, map: &HashMap<String, String>) -> String{
    match map.get(key) {
        Some(value) => value.to_string(),
        None => String::new(),
    }
}
fn fill_map(file: &mut File, map: &mut HashMap<String, String>) -> std::io::Result<()> {
    
    // Going to the top of file
    file.seek(SeekFrom::Start(0))?;
    loop {
        let mut u32_buf = [0u8; 4]; //buffer for storing lengths of key and values
        match file.read_exact(&mut u32_buf) {
            Ok(()) => {
                let key_len = u32::from_le_bytes(u32_buf) as usize; //extracting the key length as usize from byte array
                let mut key_buffer = vec![0u8; key_len]; //initialise a buffer of the size of key
                file.read_exact(&mut key_buffer)?; //reading the key and putting it in the buffer
                let key = String::from_utf8(key_buffer).unwrap(); //getting key as string
                // println!("key length: {key_len}: {}", key);
    
                file.read_exact(&mut u32_buf)?; //read the size of value
                let val_len = u32::from_le_bytes(u32_buf) as usize; //extracting value length from byte array as usize
                let mut val_buf = vec![0u8; val_len]; //initialise a buffer of the size of value
                file.read_exact(&mut val_buf)?; //reading the value and putting it in buffer
                let val = String::from_utf8(val_buf).unwrap(); //getting the value as string
                // println!("value length: {val_len}: {}", val);

                map.insert(key, val); // insert the key and value in the map
            }
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                break;
            },
            Err(e) => return Err(e),
        }; 
        
    }
    Ok(())
}

fn main() -> std::io::Result<()> {
    let mut path = PathBuf::new();
    path.push("data_file/data.db");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().read(true).write(true).create(true).open(path)?;
    put(&mut file, "user_1", "Alice")?;
    put(&mut file, "user_2", "Bob")?;
    put(&mut file, "user_3", "Key")?;
    put(&mut file, "user_4", "Mike")?;

    let mut map = HashMap::new();
    fill_map(&mut file, &mut map)?;
    println!("{}", get("user_3", &map));
    Ok(())
}