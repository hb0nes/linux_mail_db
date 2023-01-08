use axum::extract::Query;
use axum::Json;
use log::info;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use crate::mail::{Mail, MAIL_DB};

#[derive(Debug, Deserialize)]
pub struct FindMailQuery {
    email_address_filter: String,
    subject_filter: Option<String>,
}

#[derive(Serialize)]
pub struct FindMailResponse(FxHashMap<String, Vec<Mail>>);

pub async fn find_mail(query: Query<FindMailQuery>) -> axum::response::Result<Json<FindMailResponse>> {
    let mdb = MAIL_DB.lock();
    let subject_filter = query.subject_filter.clone().unwrap_or_default();
    info!("Searching mail for {} with filter {}", query.email_address_filter, subject_filter);
    let mail_db_results: FxHashMap<String, Vec<Mail>> = mdb.iter()
        .filter(|(k, _)| k.contains(&query.email_address_filter))
        .map(|(k, v)| (k.clone(), v.clone()))
        .map(|(k, mut v)| {
            if query.subject_filter.is_some() {
                v.retain(|mail| match &mail.subject {
                    Some(v) => v.to_lowercase().contains(&subject_filter.to_lowercase()),
                    None => false,
                });
            }
            (k, v)
        })
        .map(|(k, mut v)| {
            v.sort_by(|a, b| { a.line.cmp(&b.line) });
            (k, v)
        })
        .filter(|(_, v)| !v.is_empty())
        .collect();
    if !mail_db_results.is_empty() {
        Ok(Json(FindMailResponse(mail_db_results)))
    } else {
        Err(axum::response::ErrorResponse::from(
            format!("No mails found for query: {} with filter {}\n", &query.email_address_filter, subject_filter)
        ))
    }
}