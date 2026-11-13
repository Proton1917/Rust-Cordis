//! ADR-0046 §3.8：字节流由发送方拥有，接收方按帧预算逐块拉取。
#![forbid(unsafe_code)]

use cordis_sdk::{Context, Error, Result, Service};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
};

pub const MAX_CHUNK_BYTES: u32 = 4 * 1024 * 1024;

#[cordis_sdk::interface(name = "cordis.byte-stream", version = "1.0")]
pub trait ByteStream: Send + Sync {
    async fn read(&self, max_bytes: u32) -> Result<Option<Vec<u8>>>;
}

/// ADR-0046 §3.8：参数属于每次读取的帧契约，零长度及超预算请求返回参数错误。
pub fn read_size(max_bytes: u32) -> Result<usize> {
    if max_bytes == 0 || max_bytes > MAX_CHUNK_BYTES {
        return Err(Error::new(2, "字节流读取长度需要为 1 至 4194304"));
    }
    Ok(max_bytes as usize)
}

struct Reader<R>(Mutex<Option<R>>);

#[cordis_sdk::interface]
impl<R: Read + Send + 'static> ByteStream for Reader<R> {
    async fn read(&self, max_bytes: u32) -> Result<Option<Vec<u8>>> {
        let mut bytes = vec![0; read_size(max_bytes)?];
        let mut state = self.0.lock().unwrap();
        let Some(reader) = state.as_mut() else {
            return Ok(None);
        };
        match reader.read(&mut bytes) {
            Ok(0) => {
                state.take();
                Ok(None)
            }
            Ok(count) => {
                bytes.truncate(count);
                Ok(Some(bytes))
            }
            Err(problem) => {
                state.take();
                Err(error(problem))
            }
        }
    }
}

/// ADR-0046 §3.8：文件、套接字或迭代器适配为拥有型对象，最后一份引用释放底层资源。
pub fn from_reader<R: Read + Send + 'static>(
    context: &Context,
    reader: R,
) -> Result<Service<dyn ByteStream>> {
    context.service::<dyn ByteStream>(Arc::new(Reader(Mutex::new(Some(reader)))))
}

pub fn from_file(context: &Context, path: impl AsRef<Path>) -> Result<Service<dyn ByteStream>> {
    from_reader(context, File::open(path).map_err(error)?)
}

struct Chunks<I> {
    chunks: I,
    current: std::io::Cursor<Vec<u8>>,
}

impl<I: Iterator<Item = std::io::Result<Vec<u8>>>> Read for Chunks<I> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let count = self.current.read(buffer)?;
            if count != 0 {
                return Ok(count);
            }
            match self.chunks.next() {
                Some(bytes) => self.current = std::io::Cursor::new(bytes?),
                None => return Ok(0),
            }
        }
    }
}

pub fn from_chunks<I>(context: &Context, chunks: I) -> Result<Service<dyn ByteStream>>
where
    I: Iterator<Item = std::io::Result<Vec<u8>>> + Send + 'static,
{
    from_reader(
        context,
        Chunks {
            chunks,
            current: std::io::Cursor::new(Vec::new()),
        },
    )
}

/// ADR-0046 §3.8：按调用方选择的固定块长读取，写入失败时立即归还流引用。
pub async fn copy_to<W: Write>(
    source: Service<dyn ByteStream>,
    writer: &mut W,
    chunk_bytes: u32,
) -> Result<u64> {
    read_size(chunk_bytes)?;
    let mut total = 0;
    while let Some(bytes) = source.read(chunk_bytes).await? {
        if bytes.is_empty() || bytes.len() > chunk_bytes as usize {
            return Err(Error::new(2, "字节流块长度超出本次读取契约"));
        }
        writer.write_all(&bytes).map_err(error)?;
        total += bytes.len() as u64;
    }
    Ok(total)
}

pub async fn to_file(
    source: Service<dyn ByteStream>,
    path: impl AsRef<Path>,
    chunk_bytes: u32,
) -> Result<u64> {
    let mut file = File::create(path).map_err(error)?;
    copy_to(source, &mut file, chunk_bytes).await
}

fn error(problem: std::io::Error) -> Error {
    Error::new(1, problem.to_string())
}
