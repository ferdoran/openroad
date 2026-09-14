use std::fs::File;
use std::io::{self, BufReader, Read};
use std::os::unix::prelude::FileExt;
use std::path::{Path, PathBuf};

use image::io::Reader as ImageReader;

pub fn read_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let f = File::open(path)?;
    let mut reader = BufReader::new(f);
    let mut buffer = Vec::new();

    // Read file into vector.
    reader.read_to_end(&mut buffer)?;

    Ok(buffer)
}

pub fn write_bytes(path: &Path, data: &[u8]) -> io::Result<usize> {
    let file_match = File::create(path);

    let file = match file_match {
        Ok(file) => file,
        Err(error) => panic!("Problem creating the file: {:?}", error),
    };

    file.write_at(data, 0)
}

pub fn to_dds_bytes(bytes: Vec<u8>) -> Vec<u8> {
    // credits to bheaven @ https://www.elitepvpers.com/forum/sro-private-server/1542282-question-ddj-convert.html
    // skip DDJ header with the size of 20
    bytes[20..].to_vec()
}
pub fn to_dds(path: PathBuf, out: PathBuf) -> io::Result<usize> {
    let result = read_bytes(path.as_path());
    let bytes = result.expect("i failed");
    let dds_bytes = to_dds_bytes(bytes);
    let dds_path = out.with_extension("dds");
    write_bytes(&dds_path, &dds_bytes)
}
pub fn to_bmp(path: PathBuf, out: PathBuf) -> io::Result<usize> {
    let result = read_bytes(path.as_path());
    let bytes = result.expect("i failed");
    println!("File {:?} Bytes {:?}", path, bytes.len());
    //let img_result = image::load_from_memory_with_format(&bytes[20..], ImageFormat::Dds);
    let img_result = ImageReader::open(path.with_extension("dds"))?.decode();
    match img_result {
        Ok(image) => image.save(out.with_extension("bmp")),
        Err(e) => panic!("Error: {:?}", e),
    }
    .expect("TODO: panic message");
    Result::Ok(0)
}

fn to_ktx2() { /* Not implemented yet */
}
fn to_png() { /* Not implemented yet */
}

pub fn from_dds(path: PathBuf, out: PathBuf) -> io::Result<usize> {
    let result = read_bytes(path.as_path());
    let mut bytes = result.expect("i failed");
    let ddj_path = out.with_extension("ddj");

    // credits to bheaven @ https://www.elitepvpers.com/forum/sro-private-server/1542282-question-ddj-convert.html
    // and to https://github.com/JellyBitz/JMXVDDJConverter
    // "JMXVDDJ 1000" as bytes
    let mut header1 = vec![
        0x4A, 0x4D, 0x58, 0x56, 0x44, 0x44, 0x4A, 0x20, 0x31, 0x30, 0x30, 0x30,
    ];
    let mut header2 = i32::to_ne_bytes(bytes.len() as i32 + 8).to_vec();
    let mut header3 = i32::to_ne_bytes(3).to_vec();
    let total_size: usize = header1.len() + header2.len() + header3.len() + bytes.len();
    let mut dds_bytes: Vec<u8> = Vec::with_capacity(total_size);
    dds_bytes.append(&mut header1);
    dds_bytes.append(&mut header2);
    dds_bytes.append(&mut header3); // 3 = texture
    dds_bytes.append(&mut bytes);

    write_bytes(&ddj_path, &dds_bytes)
}
pub fn from_bmp(_path: PathBuf, _out: PathBuf) -> io::Result<usize> {
    panic!("Not implemented yet!")
}
