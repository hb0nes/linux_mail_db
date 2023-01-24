use std::fs::File;
use std::io;
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, format_err};
use log::{debug, error, info, warn};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::mail::FileLines;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ParseEventError {
    #[error("the event {:?} is unhandled", 0)]
    UnhandledEvent(EventKind),
    #[error("FileReading error occurred during event parsing: {source}", )]
    FileReading {
        source: anyhow::Error,
    },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<io::Error> for ParseEventError {
    fn from(value: io::Error) -> Self {
        ParseEventError::FileReading { source: value.into() }
    }
}

impl From<notify::Error> for ParseEventError {
    fn from(value: notify::Error) -> Self {
        ParseEventError::FileReading { source: value.into() }
    }
}

pub struct FileTail {
    pos: u64,
    file_path: PathBuf,
    rx_fs_events: mpsc::UnboundedReceiver<notify::Result<Event>>,
    #[allow(dead_code)]
    watcher: RecommendedWatcher,
    tx_lines: mpsc::Sender<FileLines>,
}

impl FileTail {
    // Start watching file and reset position to the beginning of the file
    // to read file from the start on the next Notify event
    fn reset(&mut self) -> anyhow::Result<(), notify::Error> {
        self.pos = 0;
        self.watcher.watch(&self.file_path, RecursiveMode::NonRecursive)?;
        Ok(())
    }

    // Retry tailing the file
    async fn retry(&mut self, retries: i32, interval: Duration) -> anyhow::Result<()> {
        for i in 0..retries {
            tokio::time::sleep(interval).await;
            warn!("Retry attempt {}/{}...", i+1, retries);
            match self.reset() {
                Ok(_) => {
                    info!("Tailing file {} successfully.", &self.file_path.display());
                    return Ok(());
                }
                Err(why) => warn!("{why}")
            }
        }
        bail!("Retrying to tail file failed.")
    }

    // Open file and read lines from given position
    fn read(&mut self) -> anyhow::Result<FileLines, io::Error> {
        let mut file = File::open(&self.file_path)?;
        let file_size = file.metadata()?.len();
        file.seek(SeekFrom::Start(self.pos))?;
        let reader = FileLines::from(file);
        self.pos = file_size;
        Ok(reader)
    }

    pub fn new(file_path: &PathBuf) -> anyhow::Result<(Self, mpsc::Receiver<FileLines>)> {
        let file = File::open(file_path).with_context(|| "when creating new FileTail")?;
        let pos = file.metadata()?.len();
        let file_path = file_path.clone();
        let (tx_fs_events, rx_fs_events) = mpsc::unbounded_channel();
        let (tx_lines, rx_lines) = mpsc::channel(5);
        let mut watcher: RecommendedWatcher = RecommendedWatcher::new(move |res| {
            tx_fs_events.send(res).unwrap()
        }, Config::default())?;
        watcher.watch(&file_path, RecursiveMode::NonRecursive)?;
        let file_tail = FileTail {
            pos,
            file_path,
            rx_fs_events,
            watcher,
            tx_lines,
        };
        Ok((file_tail, rx_lines))
    }

    pub async fn tail(&mut self) -> anyhow::Result<String> {
        while let Some(event) = self.rx_fs_events.recv().await {
            match self.parse_event(event) {
                Ok(lines) => {
                    if let Some(l) = lines && let Err(why) = self.tx_lines.send(l).await {
                        bail!("error while sending lines from FileTail event watcher: {why}");
                    }
                }
                Err(why) => match why {
                    ParseEventError::FileReading { .. } => {
                        warn!("{why:?}");
                        self.retry(5, Duration::from_secs(5)).await?;
                    }
                    ParseEventError::Other(why) => warn!("Unknown event parser error occurred: {why:?}"),
                    ParseEventError::UnhandledEvent(kind) => debug!("unhandled event: {:?}", kind),
                }
            }
        }
        bail!("Ended task that is tailing file: {:?}.", self.file_path)
    }

    fn parse_event(&mut self, event: notify::Result<Event>) -> anyhow::Result<Option<FileLines>, ParseEventError> {
        match event {
            Ok(e) => {
                if e.kind.is_create() || e.kind.is_remove() {
                    // If file was removed or created (logrotate), reset position to the beginning of the new file
                    self.reset()?;
                    let file_lines = self.read()?;
                    Ok(Some(file_lines))
                } else if e.kind.is_modify() {
                    // Otherwise, seek to new lines and return a buffered reader over those new lines
                    let file_lines = self.read()?;
                    Ok(Some(file_lines))
                } else if e.kind.is_access() {
                    Ok(None)
                } else {
                    Err(ParseEventError::UnhandledEvent(e.kind))
                }
            }
            Err(why) => Err(ParseEventError::Other(why.into())),
        }
    }
