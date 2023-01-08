use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;

use anyhow::{Context, format_err};
use log::error;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use notify::event::{CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind, RenameMode};
use tokio::sync::mpsc;

use crate::mail::FileLines;

pub struct FileTail {
    pos: u64,
    file_path: PathBuf,
    rx_fs_events: mpsc::UnboundedReceiver<notify::Result<Event>>,
    #[allow(dead_code)]
    watcher: RecommendedWatcher,
    tx_lines: mpsc::Sender<FileLines>,
}

impl FileTail {
    pub fn new(file_path: &PathBuf) -> anyhow::Result<(Self, mpsc::Receiver<FileLines>)> {
        let file = File::open(file_path).with_context(|| "when creating new FileTail")?;
        let pos = file.metadata()?.len();
        let (tx_lines, rx_lines) = mpsc::channel::<FileLines>(5);
        let (tx_watcher, rx_fs_events) = mpsc::unbounded_channel();
        let mut watcher: RecommendedWatcher = RecommendedWatcher::new(move |res| {
            tx_watcher.send(res).unwrap();
        }, Config::default())?;
        watcher.watch(file_path, RecursiveMode::NonRecursive)?;
        let file_path = file_path.clone();
        let file_tail = FileTail {
            pos,
            file_path,
            tx_lines,
            watcher,
            rx_fs_events,
        };
        Ok((file_tail, rx_lines))
    }

    pub async fn tail(&mut self) -> anyhow::Result<String> {
        while let Some(event) = self.rx_fs_events.recv().await {
            let lines = self.parse_event(event);
            if let Some(l) = lines {
                if let Err(why) = self.tx_lines.send(l).await {
                    return Err(format_err!("error while sending lines from FileTail event watcher: {why}"));
                }
            }
        }
        Ok(format!("Ended task that is tailing file: {:?}.", self.file_path))
    }

    fn parse_event(&mut self, event: notify::Result<Event>) -> Option<FileLines> {
        match event {
            Ok(e) => {
                // If file was rotated by moving, removed or freshly created, reset position to the beginning of the new file
                match e.kind {
                    EventKind::Modify(ModifyKind::Name(RenameMode::Any)) | EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any))
                    | EventKind::Remove(RemoveKind::Any) | EventKind::Remove(RemoveKind::File)
                    | EventKind::Create(CreateKind::Any) | EventKind::Create(CreateKind::File)
                    => {
                        self.pos = 0;
                        return None;
                    }
                    _ => {}
                }
                // File was modified.
                // Read file from last known position to read new changes.
                if let EventKind::Modify(ModifyKind::Data(DataChange::Content)) = e.kind {
                    // If currently opened file cannot be queried for metadata, something must've happened to it.
                    // Reset position and reopen file.
                    let mut file = File::open(&self.file_path).ok()?;
                    let file_size = file.metadata().ok()?.len();
                    file.seek(SeekFrom::Start(self.pos)).ok()?;
                    let reader = FileLines::from(file);
                    self.pos = file_size;
                    return Some(reader);
                }
            }
            Err(why) => error!("{:?}", why)
        }
        None
    }
}