use crate::at_rest::{policy_for_project, require_master_key, ProjectKeyStore};
use crate::fs_safety::{
    open_project_file_read, remove_project_file_if_exists, stage_project_file,
    write_project_file_atomic, ProjectFileStager,
};
use crate::state::AppState;
use actix_web::http::header::{
    ContentDisposition, DispositionParam, DispositionType, ACCEPT_RANGES, CONTENT_DISPOSITION,
    CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE,
};
use actix_web::{web, HttpRequest, HttpResponse};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use trueshot_storage::encrypted::SeekableEncryptedFile;

const STREAM_CHUNK_BYTES: usize = 256 * 1024;

pub(crate) trait ProjectAssetReader: Read + Seek {}
impl<T: Read + Seek> ProjectAssetReader for T {}

/// A descriptor-bound plaintext view of a project asset.
///
/// The path is retained only for MIME and filename metadata. All response bytes
/// come from the already-open clear descriptor or authenticated TSE2 reader.
pub(crate) struct OpenedProjectAsset {
    reader: Box<dyn ProjectAssetReader + Send>,
    plaintext_len: u64,
    logical_path: PathBuf,
}

impl OpenedProjectAsset {
    pub(crate) fn open(
        state: &AppState,
        project_id: &str,
        logical_path: &Path,
    ) -> Result<Self, HttpResponse> {
        match open_project_file_read(&state.config.paths.projects_dir, project_id, logical_path) {
            Ok(file) => return Self::from_clear(file, logical_path),
            Err(response) if response.status() == actix_web::http::StatusCode::NOT_FOUND => {}
            Err(response) => return Err(response),
        }

        let encrypted_path = PathBuf::from(format!("{}.enc", logical_path.display()));
        let encrypted_file = match open_project_file_read(
            &state.config.paths.projects_dir,
            project_id,
            &encrypted_path,
        ) {
            Ok(file) => file,
            Err(response) if response.status() == actix_web::http::StatusCode::NOT_FOUND => {
                return Err(HttpResponse::NotFound().body("Project asset not found"))
            }
            Err(response) => return Err(response),
        };
        let key = ProjectKeyStore::new(
            &state.config.paths.projects_dir,
            require_master_key(&state.config.privacy, &state.config.paths.projects_dir).map_err(
                |_| HttpResponse::InternalServerError().body("Project encryption key unavailable"),
            )?,
        )
        .load_or_create(project_id)
        .map_err(|_| {
            HttpResponse::InternalServerError().body("Project encryption key unavailable")
        })?;
        let reader = SeekableEncryptedFile::from_file(encrypted_file, &key).map_err(|error| {
            tracing::warn!("Rejected encrypted project asset: {error}");
            HttpResponse::UnprocessableEntity()
                .body("Encrypted project asset is invalid or requires migration")
        })?;
        let plaintext_len = reader.plaintext_len();
        Ok(Self {
            reader: Box::new(reader),
            plaintext_len,
            logical_path: logical_path.to_path_buf(),
        })
    }

    fn from_clear(file: File, logical_path: &Path) -> Result<Self, HttpResponse> {
        let plaintext_len = file
            .metadata()
            .map_err(|_| HttpResponse::InternalServerError().body("Project asset metadata failed"))?
            .len();
        Ok(Self {
            reader: Box::new(file),
            plaintext_len,
            logical_path: logical_path.to_path_buf(),
        })
    }

    pub(crate) fn read_to_end_bounded(mut self, max_bytes: usize) -> Result<Vec<u8>, HttpResponse> {
        if self.plaintext_len > max_bytes as u64 {
            return Err(HttpResponse::PayloadTooLarge().body("Project asset exceeds read limit"));
        }
        let mut bytes = Vec::with_capacity(self.plaintext_len as usize);
        self.reader
            .by_ref()
            .take(max_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| HttpResponse::InternalServerError().body("Project asset read failed"))?;
        if bytes.len() > max_bytes {
            return Err(HttpResponse::PayloadTooLarge().body("Project asset exceeds read limit"));
        }
        Ok(bytes)
    }

    pub(crate) fn into_reader(self) -> Box<dyn ProjectAssetReader + Send> {
        self.reader
    }

    pub(crate) fn plaintext_len(&self) -> u64 {
        self.plaintext_len
    }

    pub(crate) fn into_response(
        mut self,
        request: &HttpRequest,
        query_range: Option<(u64, Option<u64>)>,
        force_download: bool,
    ) -> HttpResponse {
        let requested_range = if let Some((offset, limit)) = query_range {
            match offset_limit_range(self.plaintext_len, offset, limit) {
                Ok(range) => Some(range),
                Err(()) => return range_not_satisfiable(self.plaintext_len),
            }
        } else if let Some(header) = request.headers().get(RANGE) {
            let value = match header.to_str() {
                Ok(value) => value,
                Err(_) => return HttpResponse::BadRequest().body("Invalid Range header"),
            };
            match parse_range_header(value, self.plaintext_len) {
                Ok(range) => Some(range),
                Err(()) => return range_not_satisfiable(self.plaintext_len),
            }
        } else {
            None
        };

        let (offset, length, partial) = match requested_range {
            Some((start, end)) => (start, end - start + 1, true),
            None => (0, self.plaintext_len, false),
        };
        if self.reader.seek(SeekFrom::Start(offset)).is_err() {
            return HttpResponse::InternalServerError().body("Project asset seek failed");
        }

        let filename = self
            .logical_path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "asset".to_string());
        let mime = mime_guess::from_path(&self.logical_path).first_or_octet_stream();
        let disposition = ContentDisposition {
            disposition: if force_download || !mime_allows_inline(&mime) {
                DispositionType::Attachment
            } else {
                DispositionType::Inline
            },
            parameters: vec![DispositionParam::Filename(filename)],
        };
        let stream = bounded_reader_stream(self.reader, length);
        let mut response = if partial {
            HttpResponse::PartialContent()
        } else {
            HttpResponse::Ok()
        };
        response.insert_header((CONTENT_TYPE, mime.to_string()));
        response.insert_header((CONTENT_LENGTH, length.to_string()));
        response.insert_header((ACCEPT_RANGES, "bytes"));
        response.insert_header((CONTENT_DISPOSITION, disposition));
        if partial {
            response.insert_header((
                CONTENT_RANGE,
                format!(
                    "bytes {}-{}/{}",
                    offset,
                    offset + length - 1,
                    self.plaintext_len
                ),
            ));
        }
        response.streaming(stream)
    }
}

pub(crate) fn write_project_asset_bytes(
    state: &AppState,
    project_id: &str,
    logical_path: &Path,
    bytes: &[u8],
) -> Result<(), HttpResponse> {
    if policy_for_project(
        &state.config.paths.projects_dir,
        project_id,
        &state.config.privacy,
    )
    .is_some()
    {
        let key = ProjectKeyStore::new(
            &state.config.paths.projects_dir,
            require_master_key(&state.config.privacy, &state.config.paths.projects_dir).map_err(
                |_| HttpResponse::InternalServerError().body("Project encryption key unavailable"),
            )?,
        )
        .load_or_create(project_id)
        .map_err(|_| {
            HttpResponse::InternalServerError().body("Project encryption key unavailable")
        })?;
        let encrypted_path = PathBuf::from(format!("{}.enc", logical_path.display()));
        let mut staged = stage_project_file(
            &state.config.paths.projects_dir,
            project_id,
            &encrypted_path,
            true,
        )?;
        trueshot_storage::encrypted::encrypt_bytes_to_writer(
            staged.file_mut(),
            &key,
            bytes,
            trueshot_storage::encrypted::DEFAULT_CHUNK_SIZE,
        )
        .map_err(|error| {
            tracing::warn!("Failed to encrypt project asset: {error}");
            HttpResponse::InternalServerError().body("Failed to encrypt project asset")
        })?;
        staged.commit()?;
        remove_project_file_if_exists(&state.config.paths.projects_dir, project_id, logical_path)?;
    } else {
        write_project_file_atomic(
            &state.config.paths.projects_dir,
            project_id,
            logical_path,
            bytes,
        )?;
        let encrypted_path = PathBuf::from(format!("{}.enc", logical_path.display()));
        remove_project_file_if_exists(
            &state.config.paths.projects_dir,
            project_id,
            &encrypted_path,
        )?;
    }
    Ok(())
}

pub(crate) fn commit_project_asset_stager(
    state: &AppState,
    project_id: &str,
    logical_path: &Path,
    mut plaintext: ProjectFileStager,
) -> Result<(), HttpResponse> {
    if policy_for_project(
        &state.config.paths.projects_dir,
        project_id,
        &state.config.privacy,
    )
    .is_some()
    {
        let key = ProjectKeyStore::new(
            &state.config.paths.projects_dir,
            require_master_key(&state.config.privacy, &state.config.paths.projects_dir).map_err(
                |_| HttpResponse::InternalServerError().body("Project encryption key unavailable"),
            )?,
        )
        .load_or_create(project_id)
        .map_err(|_| {
            HttpResponse::InternalServerError().body("Project encryption key unavailable")
        })?;
        let plaintext_len = plaintext
            .file_mut()
            .metadata()
            .map_err(|_| {
                HttpResponse::InternalServerError().body("Project output metadata failed")
            })?
            .len();
        plaintext
            .file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(|_| HttpResponse::InternalServerError().body("Project output seek failed"))?;

        let encrypted_path = PathBuf::from(format!("{}.enc", logical_path.display()));
        let mut encrypted = stage_project_file(
            &state.config.paths.projects_dir,
            project_id,
            &encrypted_path,
            true,
        )?;
        trueshot_storage::encrypted::encrypt_reader_to_writer(
            plaintext.file_mut(),
            encrypted.file_mut(),
            plaintext_len,
            &key,
            trueshot_storage::encrypted::DEFAULT_CHUNK_SIZE,
        )
        .map_err(|error| {
            tracing::warn!("Failed to encrypt staged project asset: {error}");
            HttpResponse::InternalServerError().body("Failed to encrypt project asset")
        })?;
        encrypted.commit()?;
        remove_project_file_if_exists(&state.config.paths.projects_dir, project_id, logical_path)?;
    } else {
        plaintext.commit()?;
        let encrypted_path = PathBuf::from(format!("{}.enc", logical_path.display()));
        remove_project_file_if_exists(
            &state.config.paths.projects_dir,
            project_id,
            &encrypted_path,
        )?;
    }
    Ok(())
}

fn bounded_reader_stream(
    reader: Box<dyn ProjectAssetReader + Send>,
    length: u64,
) -> impl futures::Stream<Item = Result<web::Bytes, std::io::Error>> {
    async_stream::try_stream! {
        let mut reader = reader;
        let mut remaining = length;
        while remaining > 0 {
            let chunk_len = remaining.min(STREAM_CHUNK_BYTES as u64) as usize;
            let (returned_reader, result) = tokio::task::spawn_blocking(move || {
                let mut reader = reader;
                let mut bytes = vec![0u8; chunk_len];
                let result = reader.read(&mut bytes).map(|count| {
                    bytes.truncate(count);
                    bytes
                });
                (reader, result)
            })
            .await
            .map_err(|_| std::io::Error::other("Project asset reader task failed"))?;
            reader = returned_reader;
            let bytes = result?;
            if bytes.is_empty() {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Project asset ended before its declared length",
                ))?;
            }
            remaining -= bytes.len() as u64;
            yield web::Bytes::from(bytes);
        }
    }
}

fn mime_allows_inline(mime: &mime_guess::mime::Mime) -> bool {
    use mime_guess::mime;
    matches!(
        mime.type_(),
        mime::IMAGE | mime::TEXT | mime::AUDIO | mime::VIDEO
    ) || (mime.type_() == mime::APPLICATION
        && matches!(
            mime.subtype().as_str(),
            "javascript" | "json" | "wasm" | "xhtml+xml"
        ))
}

fn offset_limit_range(file_size: u64, offset: u64, limit: Option<u64>) -> Result<(u64, u64), ()> {
    if file_size == 0 || offset >= file_size {
        return Err(());
    }
    let end_exclusive = limit
        .map(|limit| offset.saturating_add(limit))
        .unwrap_or(file_size)
        .min(file_size);
    if end_exclusive <= offset {
        return Err(());
    }
    Ok((offset, end_exclusive - 1))
}

pub(crate) fn parse_range_header(value: &str, file_size: u64) -> Result<(u64, u64), ()> {
    let value = value.trim();
    let ranges = value.strip_prefix("bytes=").ok_or(())?;
    if ranges.contains(',') || file_size == 0 {
        return Err(());
    }
    let (start_str, end_str) = ranges.trim().split_once('-').ok_or(())?;
    let start_str = start_str.trim();
    let end_str = end_str.trim();
    if start_str.is_empty() && end_str.is_empty() {
        return Err(());
    }
    let (start, end) = if start_str.is_empty() {
        let suffix: u64 = end_str.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        (
            file_size.saturating_sub(suffix.min(file_size)),
            file_size - 1,
        )
    } else {
        let start: u64 = start_str.parse().map_err(|_| ())?;
        let end = if end_str.is_empty() {
            file_size - 1
        } else {
            end_str.parse().map_err(|_| ())?
        };
        (start, end.min(file_size - 1))
    };
    if start > end || start >= file_size {
        return Err(());
    }
    Ok((start, end))
}

fn range_not_satisfiable(file_size: u64) -> HttpResponse {
    HttpResponse::RangeNotSatisfiable()
        .insert_header((CONTENT_RANGE, format!("bytes */{file_size}")))
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parser_accepts_single_bounded_open_and_suffix_ranges() {
        assert_eq!(parse_range_header("bytes=2-5", 10), Ok((2, 5)));
        assert_eq!(parse_range_header("bytes=7-", 10), Ok((7, 9)));
        assert_eq!(parse_range_header("bytes=-3", 10), Ok((7, 9)));
        assert_eq!(parse_range_header("bytes=-99", 10), Ok((0, 9)));
    }

    #[test]
    fn range_parser_rejects_empty_zero_length_and_multi_ranges() {
        assert!(parse_range_header("bytes=-0", 10).is_err());
        assert!(parse_range_header("bytes=5-4", 10).is_err());
        assert!(parse_range_header("bytes=0-1,4-5", 10).is_err());
        assert!(parse_range_header("bytes=0-0", 0).is_err());
    }

    #[test]
    fn offset_limit_is_overflow_safe_and_bounded() {
        assert_eq!(offset_limit_range(10, 3, Some(4)), Ok((3, 6)));
        assert_eq!(offset_limit_range(10, 3, Some(u64::MAX)), Ok((3, 9)));
        assert!(offset_limit_range(10, 10, None).is_err());
        assert!(offset_limit_range(10, 3, Some(0)).is_err());
    }

    #[actix_rt::test]
    async fn descriptor_stream_returns_only_the_requested_range() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("asset.bin");
        std::fs::write(&path, b"0123456789").unwrap();
        let asset = OpenedProjectAsset::from_clear(File::open(&path).unwrap(), &path).unwrap();
        let request = actix_web::test::TestRequest::default()
            .insert_header((RANGE, "bytes=2-5"))
            .to_http_request();

        let response = asset.into_response(&request, None, false);
        assert_eq!(
            response.status(),
            actix_web::http::StatusCode::PARTIAL_CONTENT
        );
        assert_eq!(
            response
                .headers()
                .get(CONTENT_RANGE)
                .unwrap()
                .to_str()
                .unwrap(),
            "bytes 2-5/10"
        );
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        assert_eq!(body, web::Bytes::from_static(b"2345"));
    }

    #[cfg(unix)]
    #[actix_rt::test]
    async fn encrypted_stream_ignores_a_post_open_path_swap() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let logical_path = temp.path().join("asset.raw");
        let encrypted_path = temp.path().join("asset.raw.enc");
        let original_path = temp.path().join("asset.raw.original.enc");
        let attacker_path = temp.path().join("attacker.raw.enc");
        let key = [29u8; 32];
        trueshot_storage::encrypted::encrypt_bytes(
            &encrypted_path,
            &key,
            b"measured-encrypted-payload",
            64 * 1024,
        )
        .unwrap();
        trueshot_storage::encrypted::encrypt_bytes(
            &attacker_path,
            &key,
            b"attacker-encrypted-payload",
            64 * 1024,
        )
        .unwrap();
        let opened = File::open(&encrypted_path).unwrap();
        std::fs::rename(&encrypted_path, &original_path).unwrap();
        symlink(&attacker_path, &encrypted_path).unwrap();
        let reader = SeekableEncryptedFile::from_file(opened, &key).unwrap();
        let plaintext_len = reader.plaintext_len();
        let asset = OpenedProjectAsset {
            reader: Box::new(reader),
            plaintext_len,
            logical_path,
        };
        let request = actix_web::test::TestRequest::default().to_http_request();

        let response = asset.into_response(&request, Some((9, Some(9))), false);
        let body = actix_web::body::to_bytes(response.into_body())
            .await
            .unwrap();
        assert_eq!(body, web::Bytes::from_static(b"encrypted"));
        assert!(!temp.path().join("asset.raw").exists());
    }
}
