//! Thot library — shared by the desktop binary and the Android cdylib (JNI).

pub mod api;
pub mod db;
pub mod generate;
pub mod glossary;
pub mod llm;
pub mod notes;
pub mod pdf;
pub mod podcast;
pub mod schedule;
pub mod server;
pub mod source;
pub mod tts;
pub mod web_verify;

#[cfg(target_os = "android")]
pub mod android;
