use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::Duration;
use anyhow::{bail, Context, format_err, Result};
use bytelines::ByteLinesReader;
use flate2::read::GzDecoder;
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use parking_lot::{Mutex, MutexGuard};
use rustc_hash::FxHashMap;
use serde::Serialize;
use tokio::{task, time};
use crate::{Config, FileTail};

pub(crate) static MAIL_DB: Lazy<MailDB> = Lazy::new(|| {
    MailDB::new()
});

#[derive(Debug)]
pub struct MailDB(Mutex<FxHashMap<String, Vec<Mail>>>);

impl MailDB {
    pub fn new() -> Self {
        MailDB(Mutex::new(FxHashMap::default()))
    }

    pub fn lock(&self) -> MutexGuard<FxHashMap<String, Vec<Mail>>> {
        self.0.lock()
    }

    /// Loop through local MAIL_DB and find corresponding emails that have no subject
    /// and update the subject accordingly
    pub fn update_mail_subjects(&self, new_mails: Vec<Mail>) -> i32 {
        let mut hashmap_locked = self.0.lock();
        let mut updates = 0;
        for new_mail in new_mails {
            let entry = hashmap_locked.get_mut(&new_mail.to);
            match entry {
                None => { warn!("no email address found for inserting mail subjects: {}", &new_mail.to); }
                Some(db_mails) => {
                    for db_mail in db_mails {
                        if db_mail.subject.is_none() && db_mail.id == new_mail.id {
                            db_mail.subject = new_mail.subject;
                            updates += 1;
                            break;
                        }
                    }
                }
            }
        }
        updates
    }

    /// inserts given mails into local MAIL_DB
    pub fn insert_mails(&self, new_mails: Vec<Mail>) -> i32 {
        let mut updates = 0;
        let mut lock = self.0.lock();
        for new_mail in new_mails {
            // Only update mail in MAIL_DB if a mail with an identical ID does not already exist
            let db_mail_entry = lock.entry(new_mail.to.clone()).or_insert(Vec::new());
            if !db_mail_entry.iter().any(|m| m.id == new_mail.id) {
                db_mail_entry.push(new_mail.clone());
                updates += 1;
            }
        }
        updates
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Mail {
    id: String,
    pub line: Option<String>,
    pub subject: Option<String>,
    #[serde(skip)]
    to: String,
}

type DynamicIterator = Box<dyn Iterator<Item=Result<Vec<u8>, std::io::Error>> + Send>;

pub struct FileLines(DynamicIterator);

impl FileLines {
    /// Returns a line-based buffered iterator for given file,
    /// that either decompresses a .gz file or opens a regular file
    fn new(file_name: &PathBuf) -> Result<Self> {
        let f = File::open(file_name).with_context(|| format!("trying to open {}", file_name.display()))?;
        if let Some(extension) = file_name.extension() && extension == "gz" {
            Ok(FileLines(Box::new(BufReader::new(GzDecoder::new(f)).byte_lines().into_iter())))
        } else {
            Ok(FileLines(Box::new(BufReader::new(f).byte_lines().into_iter())))
        }
    }
}

impl From<File> for FileLines {
    fn from(f: File) -> Self {
        FileLines(Box::new(BufReader::new(f).byte_lines().into_iter()))
    }
}

impl FileLines {
    pub fn into_iter(self) -> DynamicIterator {
        self.0.into_iter()
    }
}

fn id_from_log_line(line: &str) -> Option<&str> {
    let split_1 = &line.split(':').take(4).map(|s| s.trim()).collect::<Vec<_>>();
    if split_1.len() != 4 {
        return None;
    }
    let id = split_1[3];
    if id.len() != 10 {
        return None;
    }
    Some(id)
}

fn email_from_log_line(line: &str) -> Option<&str> {
    let split_1 = line.split('<').take(2).collect::<Vec<_>>();
    if split_1.len() != 2 {
        return None;
    }
    let split_2 = split_1[1].split('>').take(1).collect::<Vec<_>>();
    if split_2.len() != 1 || !split_2[0].contains('@') {
        None
    } else {
        Some(split_2[0])
    }
}

async fn init_mail_log() -> Result<i32> {
    let files = &Config::global().log.files;
    let dir = &Config::global().log.dir;
    let mut inserts_total = 0;
    for file in files {
        task::yield_now().await; // Yield to be able to cancel this task
        let file_path: PathBuf = [dir, file].iter().collect();
        let reader = FileLines::new(&file_path).with_context(|| format!("getting reader for: {}", file_path.display()))?;
        info!("Loading mail logs from file: {}...", file_path.display());
        let mails = parse_mails(reader).with_context(|| format!("parsing emails for: {}", file_path.display()))?;
        inserts_total += MAIL_DB.insert_mails(mails);
    }
    Ok(inserts_total)
}

/// Parse mails from given FileLines reader and return them
pub fn parse_mails(reader: FileLines) -> Result<Vec<Mail>> {
    let mut mails: Vec<Mail> = vec![];
    let (mut email, mut id) = (String::new(), String::new());
    for line in reader.into_iter() {
        let bytes: &[u8] = &line.with_context(|| "while reading line from FileLines")?;
        let line = String::from_utf8_lossy(bytes);
        if !line.contains("postfix/smtp[") {
            continue;
        }
        email.clear();
        id.clear();
        email = match email_from_log_line(&line) {
            None => continue,
            Some(v) => v.into(),
        };
        id = match id_from_log_line(&line) {
            None => continue,
            Some(v) => v.into(),
        };
        if !email.is_empty() && !id.is_empty() {
            mails.push(Mail {
                to: email.clone(),
                id: id.clone(),
                subject: None,
                line: Some(line.to_string()),
            });
        }
    }
    Ok(mails)
}

async fn init_mail_subjects() -> Result<i32> {
    let files = &Config::global().mail.files;
    let dir = &Config::global().mail.dir;
    let mut subjects_updated = 0;
    for file in files {
        task::yield_now().await; // Yield to be able to cancel this task
        let file_path: PathBuf = [dir, file].iter().collect();
        let reader = FileLines::new(&file_path).with_context(|| format!("getting reader for {}", file_path.display()))?;
        info!("Loading mail subjects from file: {}...", file_path.display());
        let mails_with_subject = parse_mail_subjects(reader).with_context(|| format!("parsing mail subjects for {}", file_path.display()))?;
        subjects_updated += MAIL_DB.update_mail_subjects(mails_with_subject);
    }
    Ok(subjects_updated)
}

/// parses FileLines (dynamic, line-based and buffered file reader)
/// to find an email ID, an email address and a subject.
/// Update the MAIL_DB if a matching email address and ID are found
pub fn parse_mail_subjects(reader: FileLines) -> Result<Vec<Mail>> {
    let (mut id, mut subject, mut to) = (String::new(), String::new(), String::new());
    let mut mails_with_subjects: Vec<Mail> = vec![];
    let mut parse_mail = false;
    for line in reader.0 {
        let bytes: &[u8] = &line.with_context(|| "while reading line from FileLines")?;
        let line = String::from_utf8_lossy(bytes);
        // "ESMTPS id" should indicate the start of an email, so start parsing the mail
        if !parse_mail && line.contains("ESMTPS id") {
            parse_mail = true;
            id.clear();
            subject.clear();
            to.clear();
            let split = line.split_whitespace().collect::<Vec<_>>();
            id = split[split.len() - 1].to_string();
        }
        // Don't execute rest of logic if we're not parsing the email
        // i.e. if we haven't encountered ESMTPS id
        if !parse_mail { continue; }
        if to.is_empty() && line.starts_with("To: ") {
            to = line.replace("To: ", "").replace(['<', '>'], "");
        }
        if subject.is_empty() && line.starts_with("Subject: ") {
            subject = line.replace("Subject: ", "");
        }
        // if all needed vars are found, append to our list of mail subjects
        if !subject.is_empty() && !to.is_empty() && !id.is_empty() {
            parse_mail = false;
            mails_with_subjects.push(Mail {
                id: id.clone(),
                line: None,
                subject: Some(subject.clone()),
                to: to.clone(),
            });
        }
    }
    Ok(mails_with_subjects)
}

/// initialize in-memory mail database from configured mail paths
pub async fn init_mail() -> Result<String> {
    task::yield_now().await;
    // Yield to be able to cancel this task
    info!("Loading configured email into DB...");
    info!("inserted {} emails into mail DB", init_mail_log().await?);
    info!("inserted {} subjects into mail DB", init_mail_subjects().await?);
    Ok(String::from("Loading emails done."))
}

/// tail the configured mail tail file (usually /var/mail/root) and update the
/// in memory mail database with the subjects found.
/// Function needs a delay because the mail contents should be parsed some time after
/// mails have been received to line them up to logfiles.
pub async fn tail_mail(delay: Duration) -> Result<String> {
    let file_path: PathBuf = [&Config::global().mail.dir, &Config::global().mail.tail].iter().collect();
    let (mut file_tail, mut rx_lines) = FileTail::new(&file_path)
        .with_context(|| format!("when tailing mail log file: {}", file_path.display()))?;
    {
        let file_path = file_path.clone();
        tokio::spawn(async move {
            info!("Tailing mail file: {}...", file_path.display());
            while let Some(reader) = rx_lines.recv().await {
                let parse_res = parse_mail_subjects(reader).with_context(|| format!("parsing mail subjects for {}", file_path.display()));
                match parse_res {
                    Ok(mails_with_subjects) => {
                        // Seeing as two files are being tailed simultaneously, there is a large chance that the DB isn't updated yet
                        // for the new mail. Sleep for supplied duration
                        let file_path = file_path.clone();
                        task::spawn(async move {
                            time::sleep(delay).await;
                            let updates = MAIL_DB.update_mail_subjects(mails_with_subjects);
                            if updates > 0 { debug!("Updated {updates} subjects from {}", file_path.display()) };
                        });
                    }
                    Err(why) => error!("Encountered error: '{why:?}' while tailing: {}", file_path.display()),
                }
            }
        });
    }
    let res = file_tail.tail().await?;
    bail!("Stopped tailing mail file: {}. {}", file_path.display(), res)
}

/// tail the configured mail log file (usually /var/log/mail.info) and update the
/// in memory mail database accordingly.
pub async fn tail_mail_log() -> Result<String> {
    let file_path: PathBuf = [&Config::global().log.dir, &Config::global().log.tail].iter().collect();
    let (mut file_tail, mut rx_lines) = FileTail::new(&file_path)
        .with_context(|| format!("when tailing mail log file: {}", file_path.display()))?;
    {
        let file_path = file_path.clone();
        tokio::spawn(async move {
            info!("Tailing mail logfile: {}...", file_path.display());
            while let Some(reader) = rx_lines.recv().await {
                let parse_res = parse_mails(reader).with_context(|| format!("parsing emails for: {}", file_path.display()));
                match parse_res {
                    Ok(mails) => {
                        let inserts = MAIL_DB.insert_mails(mails);
                        if inserts > 0 { debug!("Inserted {inserts} mails from {}", file_path.display()) };
                    }
                    Err(why) => error!("Encountered error: '{why:?}' while tailing: {}", file_path.display()),
                }
            }
        });
    }
    let res = file_tail.tail().await?;
    bail!("Stopped tailing mail logfile: {}. {}", file_path.display(), res)
}