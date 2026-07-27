use actix_web::HttpResponse;
use std::fs::File;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use sysinfo::Disks;

const MAX_ID_LEN: usize = 128;
const MAX_FILENAME_LEN: usize = 255;

pub fn ensure_project_id(id: &str) -> Result<(), HttpResponse> {
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return Err(HttpResponse::BadRequest().body("Invalid project id length"));
    }
    if id.chars().any(|c| c.is_control()) {
        return Err(HttpResponse::BadRequest().body("Invalid project id characters"));
    }
    if id.contains('/') || id.contains('\\') || id.contains(':') {
        return Err(HttpResponse::BadRequest().body("Project id must be a simple name"));
    }
    if id == "." || id == ".." || id.contains("..") {
        return Err(HttpResponse::BadRequest().body("Project id cannot contain path traversal"));
    }
    Ok(())
}

pub fn ensure_filename(name: &str) -> Result<(), HttpResponse> {
    if name.is_empty() || name.len() > MAX_FILENAME_LEN {
        return Err(HttpResponse::BadRequest().body("Invalid filename length"));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(HttpResponse::BadRequest().body("Invalid filename characters"));
    }
    if name.contains('/') || name.contains('\\') || name.contains(':') {
        return Err(HttpResponse::BadRequest().body("Filename must be a single path segment"));
    }
    if name == "." || name == ".." || name.contains("..") {
        return Err(HttpResponse::BadRequest().body("Filename cannot contain path traversal"));
    }
    Ok(())
}

pub fn canonicalize_root(root: &Path) -> Result<PathBuf, HttpResponse> {
    if !root.exists() {
        std::fs::create_dir_all(root).map_err(|e| {
            HttpResponse::InternalServerError().body(format!("Failed to create root: {e}"))
        })?;
    }
    root.canonicalize().map_err(|e| {
        HttpResponse::InternalServerError().body(format!("Failed to resolve root: {e}"))
    })
}

pub fn resolve_project_dir(root: &Path, id: &str) -> Result<PathBuf, HttpResponse> {
    ensure_project_id(id)?;
    let root = canonicalize_root(root)?;
    let candidate = root.join(id);
    if candidate.exists() {
        let metadata = std::fs::symlink_metadata(&candidate).map_err(|error| {
            tracing::warn!("Failed to inspect project directory: {error}");
            HttpResponse::InternalServerError().body("Failed to inspect project directory")
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(HttpResponse::BadRequest().body("Project path is not a real directory"));
        }
        let canon = candidate.canonicalize().map_err(|e| {
            HttpResponse::InternalServerError().body(format!("Failed to resolve project: {e}"))
        })?;
        if !canon.starts_with(&root) {
            return Err(HttpResponse::BadRequest().body("Invalid project path"));
        }
        Ok(canon)
    } else {
        let normalized = normalize_path(&candidate);
        if !normalized.starts_with(&root) {
            return Err(HttpResponse::BadRequest().body("Invalid project path"));
        }
        Ok(normalized)
    }
}

pub fn create_project_directory(root: &Path, id: &str) -> Result<PathBuf, HttpResponse> {
    ensure_project_id(id)?;
    let root = canonicalize_root(root)?;
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::OpenOptionsExt;

        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&root)
            .map_err(open_file_error)?;
        let name = CString::new(std::ffi::OsStr::new(id).as_bytes())
            .map_err(|_| HttpResponse::BadRequest().body("Invalid project id"))?;
        let created = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) };
        if created != 0 {
            return Err(open_file_error(std::io::Error::last_os_error()));
        }
        directory.sync_all().map_err(open_file_error)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(root.join(id)).map_err(open_file_error)?;
    }
    resolve_project_dir(&root, id)
}

pub fn resolve_project_child(root: &Path, id: &str, child: &str) -> Result<PathBuf, HttpResponse> {
    let project_dir = resolve_project_dir(root, id)?;
    let candidate = project_dir.join(child);
    if candidate.exists() {
        let canon = candidate.canonicalize().map_err(|e| {
            HttpResponse::InternalServerError()
                .body(format!("Failed to resolve project child: {e}"))
        })?;
        if !canon.starts_with(&project_dir) {
            return Err(HttpResponse::BadRequest().body("Invalid project child path"));
        }
        Ok(canon)
    } else {
        let normalized = normalize_path(&candidate);
        if !normalized.starts_with(&project_dir) {
            return Err(HttpResponse::BadRequest().body("Invalid project child path"));
        }
        Ok(normalized)
    }
}

pub fn resolve_project_file(
    root: &Path,
    id: &str,
    filename: &str,
) -> Result<PathBuf, HttpResponse> {
    ensure_filename(filename)?;
    let project_dir = resolve_project_dir(root, id)?;
    let candidate = project_dir.join(filename);
    let normalized = normalize_path(&candidate);
    if !normalized.starts_with(&project_dir) {
        return Err(HttpResponse::BadRequest().body("Invalid project file path"));
    }
    Ok(normalized)
}

pub fn resolve_project_child_file(
    root: &Path,
    id: &str,
    child: &str,
    rel_path: &str,
) -> Result<PathBuf, HttpResponse> {
    if rel_path.is_empty() {
        return Err(HttpResponse::BadRequest().body("Missing file path"));
    }
    let rel = Path::new(rel_path);
    if rel.is_absolute() {
        return Err(HttpResponse::BadRequest().body("Invalid file path"));
    }
    for component in rel.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(HttpResponse::BadRequest().body("Invalid file path"));
            }
            _ => {}
        }
    }
    let project_dir = resolve_project_dir(root, id)?;
    let base = project_dir.join(child);
    let candidate = base.join(rel);
    let normalized = normalize_path(&candidate);
    if !normalized.starts_with(&base) {
        return Err(HttpResponse::BadRequest().body("Invalid project child path"));
    }
    Ok(normalized)
}

pub fn open_project_file_read(
    root: &Path,
    id: &str,
    candidate: &Path,
) -> Result<File, HttpResponse> {
    let (root, components) = project_components(root, id, candidate)?;

    #[cfg(unix)]
    {
        open_relative_nofollow(&root, &components)
    }
    #[cfg(not(unix))]
    {
        open_relative_checked(&root, &components)
    }
}

pub fn write_project_file_atomic(
    root: &Path,
    id: &str,
    candidate: &Path,
    bytes: &[u8],
) -> Result<(), HttpResponse> {
    let mut staged = stage_project_file(root, id, candidate, true)?;
    staged
        .file_mut()
        .write_all(bytes)
        .map_err(open_file_error)?;
    staged.commit()
}

/// Creates every missing directory below a project using descriptor-relative,
/// no-follow traversal. Existing components must be real directories.
pub fn ensure_project_directory(
    root: &Path,
    id: &str,
    candidate: &Path,
) -> Result<(), HttpResponse> {
    let (root, components) = project_components(root, id, candidate)?;
    #[cfg(unix)]
    {
        ensure_relative_directory(&root, &components)
    }
    #[cfg(not(unix))]
    {
        ensure_relative_directory_checked(&root, &components)
    }
}

/// Starts a descriptor-rooted file publication.
///
/// The temporary file is not reachable by the requested target name until
/// `commit`. With `replace = false`, commit fails atomically if the target
/// already exists.
pub fn stage_project_file(
    root: &Path,
    id: &str,
    candidate: &Path,
    replace: bool,
) -> Result<ProjectFileStager, HttpResponse> {
    let (root, components) = project_components(root, id, candidate)?;
    #[cfg(unix)]
    {
        ProjectFileStager::new_unix(&root, &components, replace)
    }
    #[cfg(not(unix))]
    {
        ProjectFileStager::new_checked(&root, &components, replace)
    }
}

pub fn remove_project_file_if_exists(
    root: &Path,
    id: &str,
    candidate: &Path,
) -> Result<bool, HttpResponse> {
    let (root, components) = project_components(root, id, candidate)?;
    #[cfg(unix)]
    {
        remove_relative_file(&root, &components)
    }
    #[cfg(not(unix))]
    {
        remove_relative_file_checked(&root, &components)
    }
}

pub fn remove_project_directory_tree(
    root: &Path,
    id: &str,
    candidate: &Path,
) -> Result<bool, HttpResponse> {
    let (root, components) = project_components(root, id, candidate)?;
    #[cfg(unix)]
    {
        remove_relative_directory_tree(&root, &components)
    }
    #[cfg(not(unix))]
    {
        remove_relative_directory_tree_checked(&root, &components)
    }
}

pub struct ProjectFileStager {
    file: File,
    replace: bool,
    committed: bool,
    #[cfg(unix)]
    directory: File,
    #[cfg(unix)]
    temporary_name: std::ffi::CString,
    #[cfg(unix)]
    final_name: std::ffi::CString,
    #[cfg(not(unix))]
    temporary_path: PathBuf,
    #[cfg(not(unix))]
    final_path: PathBuf,
}

impl ProjectFileStager {
    pub fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    pub fn try_clone_file(&self) -> Result<File, HttpResponse> {
        self.file.try_clone().map_err(open_file_error)
    }

    pub fn commit(mut self) -> Result<(), HttpResponse> {
        self.file.sync_all().map_err(open_file_error)?;
        #[cfg(unix)]
        self.commit_unix()?;
        #[cfg(not(unix))]
        self.commit_checked()?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for ProjectFileStager {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        #[cfg(unix)]
        unsafe {
            libc::unlinkat(
                std::os::fd::AsRawFd::as_raw_fd(&self.directory),
                self.temporary_name.as_ptr(),
                0,
            );
        }
        #[cfg(not(unix))]
        {
            let _ = std::fs::remove_file(&self.temporary_path);
        }
    }
}

fn project_components(
    root: &Path,
    id: &str,
    candidate: &Path,
) -> Result<(PathBuf, Vec<std::ffi::OsString>), HttpResponse> {
    ensure_project_id(id)?;
    let lexical_project_dir = root.join(id);
    let root = canonicalize_root(root)?;
    let canonical_project_dir = root.join(id);
    let relative = candidate
        .strip_prefix(&lexical_project_dir)
        .or_else(|_| candidate.strip_prefix(&canonical_project_dir))
        .map_err(|_| HttpResponse::BadRequest().body("Invalid project file path"))?;
    let mut components = vec![std::ffi::OsString::from(id)];
    for component in relative.components() {
        match component {
            Component::Normal(value) => components.push(value.to_os_string()),
            _ => return Err(HttpResponse::BadRequest().body("Invalid project file path")),
        }
    }
    if components.len() < 2 {
        return Err(HttpResponse::BadRequest().body("Missing project file path"));
    }
    Ok((root, components))
}

#[cfg(unix)]
fn open_relative_nofollow(
    root: &Path,
    components: &[std::ffi::OsString],
) -> Result<File, HttpResponse> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)
        .map_err(open_file_error)?;

    for (index, component) in components.iter().enumerate() {
        let name = CString::new(component.as_os_str().as_bytes())
            .map_err(|_| HttpResponse::BadRequest().body("Invalid project file path"))?;
        let final_component = index + 1 == components.len();
        let flags = if final_component {
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK
        } else {
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY
        };
        // SAFETY: `directory` owns a valid directory descriptor, `name` is a
        // NUL-terminated single component, and the returned descriptor is
        // immediately transferred into `File`.
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(open_file_error(std::io::Error::last_os_error()));
        }
        // SAFETY: a successful `openat` returns a new owned descriptor.
        let opened = unsafe { File::from_raw_fd(descriptor) };
        if final_component {
            let metadata = opened.metadata().map_err(open_file_error)?;
            if !metadata.is_file() {
                return Err(HttpResponse::BadRequest().body("Project asset is not a regular file"));
            }
            if metadata.nlink() != 1 {
                return Err(
                    HttpResponse::BadRequest().body("Hard-linked project assets are not allowed")
                );
            }
            return Ok(opened);
        }
        directory = opened;
    }
    Err(HttpResponse::BadRequest().body("Missing project file path"))
}

#[cfg(unix)]
fn open_parent_nofollow(
    root: &Path,
    components: &[std::ffi::OsString],
) -> Result<File, HttpResponse> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)
        .map_err(open_file_error)?;
    for component in &components[..components.len() - 1] {
        let name = CString::new(component.as_os_str().as_bytes())
            .map_err(|_| HttpResponse::BadRequest().body("Invalid project file path"))?;
        // SAFETY: `directory` is a valid directory descriptor and `name` is one
        // NUL-terminated path component.
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            )
        };
        if descriptor < 0 {
            return Err(open_file_error(std::io::Error::last_os_error()));
        }
        // SAFETY: a successful `openat` returns a new owned descriptor.
        directory = unsafe { File::from_raw_fd(descriptor) };
    }
    Ok(directory)
}

#[cfg(unix)]
fn remove_relative_directory_tree(
    root: &Path,
    components: &[std::ffi::OsString],
) -> Result<bool, HttpResponse> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let parent = open_parent_nofollow(root, components)?;
    let name = CString::new(
        components
            .last()
            .expect("validated project directory component")
            .as_os_str()
            .as_bytes(),
    )
    .map_err(|_| HttpResponse::BadRequest().body("Invalid project directory path"))?;
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
        )
    };
    if descriptor < 0 {
        let open_error = std::io::Error::last_os_error();
        if open_error.kind() == std::io::ErrorKind::NotFound {
            return Ok(false);
        }
        let removed = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) };
        if removed == 0 {
            parent.sync_all().map_err(open_file_error)?;
            return Ok(true);
        }
        return Err(open_file_error(std::io::Error::last_os_error()));
    }
    let directory = unsafe { File::from_raw_fd(descriptor) };
    clear_open_directory_nofollow(&directory, 0)?;
    let removed = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
    if removed != 0 {
        return Err(open_file_error(std::io::Error::last_os_error()));
    }
    parent.sync_all().map_err(open_file_error)?;
    Ok(true)
}

#[cfg(unix)]
fn clear_open_directory_nofollow(directory: &File, depth: usize) -> Result<(), HttpResponse> {
    use std::ffi::{CStr, CString};
    use std::os::fd::{AsRawFd, FromRawFd};

    if depth >= 128 {
        return Err(HttpResponse::PayloadTooLarge().body("Project directory nesting is too deep"));
    }

    struct DirectoryStream(*mut libc::DIR);
    impl Drop for DirectoryStream {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }

    let duplicate = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(open_file_error(std::io::Error::last_os_error()));
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        unsafe {
            libc::close(duplicate);
        }
        return Err(open_file_error(std::io::Error::last_os_error()));
    }
    let stream = DirectoryStream(stream);

    loop {
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        let name = CString::new(name)
            .map_err(|_| HttpResponse::BadRequest().body("Invalid project entry name"))?;
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            )
        };
        if descriptor >= 0 {
            let child = unsafe { File::from_raw_fd(descriptor) };
            clear_open_directory_nofollow(&child, depth + 1)?;
            let removed =
                unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
            if removed != 0 {
                return Err(open_file_error(std::io::Error::last_os_error()));
            }
        } else {
            let removed = unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
            if removed != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(open_file_error(error));
                }
            }
        }
    }
    directory.sync_all().map_err(open_file_error)
}

#[cfg(unix)]
fn ensure_relative_directory(
    root: &Path,
    components: &[std::ffi::OsString],
) -> Result<(), HttpResponse> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)
        .map_err(open_file_error)?;
    for component in components {
        let name = CString::new(component.as_os_str().as_bytes())
            .map_err(|_| HttpResponse::BadRequest().body("Invalid project directory path"))?;
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            )
        };
        let descriptor = if descriptor >= 0 {
            descriptor
        } else {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(open_file_error(error));
            }
            let created = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) };
            if created != 0 {
                let create_error = std::io::Error::last_os_error();
                if create_error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(open_file_error(create_error));
                }
            }
            let opened = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
                )
            };
            if opened < 0 {
                return Err(open_file_error(std::io::Error::last_os_error()));
            }
            directory.sync_all().map_err(open_file_error)?;
            opened
        };
        directory = unsafe { File::from_raw_fd(descriptor) };
    }
    directory.sync_all().map_err(open_file_error)
}

#[cfg(unix)]
fn remove_relative_file(
    root: &Path,
    components: &[std::ffi::OsString],
) -> Result<bool, HttpResponse> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    let directory = open_parent_nofollow(root, components)?;
    let final_name = CString::new(
        components
            .last()
            .expect("validated non-empty component list")
            .as_os_str()
            .as_bytes(),
    )
    .map_err(|_| HttpResponse::BadRequest().body("Invalid project file path"))?;
    let removed = unsafe { libc::unlinkat(directory.as_raw_fd(), final_name.as_ptr(), 0) };
    if removed != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(false);
        }
        return Err(open_file_error(error));
    }
    directory.sync_all().map_err(open_file_error)?;
    Ok(true)
}

#[cfg(unix)]
impl ProjectFileStager {
    fn new_unix(
        root: &Path,
        components: &[std::ffi::OsString],
        replace: bool,
    ) -> Result<Self, HttpResponse> {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;

        let directory = open_parent_nofollow(root, components)?;
        let final_name = CString::new(
            components
                .last()
                .expect("validated non-empty component list")
                .as_os_str()
                .as_bytes(),
        )
        .map_err(|_| HttpResponse::BadRequest().body("Invalid project file path"))?;
        let temporary_name = CString::new(format!(
            ".trueshot-write-{}.part",
            uuid::Uuid::new_v4().as_simple()
        ))
        .expect("generated temporary filename contains no NUL");
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                temporary_name.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if descriptor < 0 {
            return Err(open_file_error(std::io::Error::last_os_error()));
        }
        let file = unsafe { File::from_raw_fd(descriptor) };
        Ok(Self {
            file,
            replace,
            committed: false,
            directory,
            temporary_name,
            final_name,
        })
    }

    fn commit_unix(&mut self) -> Result<(), HttpResponse> {
        use std::os::fd::AsRawFd;
        let result = if self.replace {
            unsafe {
                libc::renameat(
                    self.directory.as_raw_fd(),
                    self.temporary_name.as_ptr(),
                    self.directory.as_raw_fd(),
                    self.final_name.as_ptr(),
                )
            }
        } else {
            let linked = unsafe {
                libc::linkat(
                    self.directory.as_raw_fd(),
                    self.temporary_name.as_ptr(),
                    self.directory.as_raw_fd(),
                    self.final_name.as_ptr(),
                    0,
                )
            };
            if linked == 0 {
                unsafe {
                    libc::unlinkat(self.directory.as_raw_fd(), self.temporary_name.as_ptr(), 0);
                };
                0
            } else {
                linked
            }
        };
        if result != 0 {
            return Err(open_file_error(std::io::Error::last_os_error()));
        }
        self.directory.sync_all().map_err(open_file_error)
    }
}

#[cfg(not(unix))]
fn open_relative_checked(
    root: &Path,
    components: &[std::ffi::OsString],
) -> Result<File, HttpResponse> {
    let mut path = root.to_path_buf();
    for component in components {
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path).map_err(open_file_error)?;
        if metadata.file_type().is_symlink() {
            return Err(HttpResponse::BadRequest().body("Project symlinks are not allowed"));
        }
    }
    let file = File::open(path).map_err(open_file_error)?;
    if !file.metadata().map_err(open_file_error)?.is_file() {
        return Err(HttpResponse::BadRequest().body("Project asset is not a regular file"));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn write_relative_checked(
    root: &Path,
    components: &[std::ffi::OsString],
    bytes: &[u8],
) -> Result<(), HttpResponse> {
    let mut path = root.to_path_buf();
    for component in &components[..components.len() - 1] {
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path).map_err(open_file_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(HttpResponse::BadRequest().body("Unsafe project asset path"));
        }
    }
    let final_path = path.join(
        components
            .last()
            .expect("validated non-empty component list"),
    );
    let temporary = path.join(format!(
        ".trueshot-write-{}.part",
        uuid::Uuid::new_v4().as_simple()
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &final_path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(open_file_error)
}

#[cfg(not(unix))]
fn ensure_relative_directory_checked(
    root: &Path,
    components: &[std::ffi::OsString],
) -> Result<(), HttpResponse> {
    let mut path = root.to_path_buf();
    for component in components {
        path.push(component);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(HttpResponse::BadRequest().body("Unsafe project directory path"))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&path).map_err(open_file_error)?;
            }
            Err(error) => return Err(open_file_error(error)),
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn remove_relative_file_checked(
    root: &Path,
    components: &[std::ffi::OsString],
) -> Result<bool, HttpResponse> {
    let mut path = root.to_path_buf();
    for component in &components[..components.len() - 1] {
        path.push(component);
        let metadata = std::fs::symlink_metadata(&path).map_err(open_file_error)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(HttpResponse::BadRequest().body("Unsafe project asset path"));
        }
    }
    path.push(
        components
            .last()
            .expect("validated non-empty component list"),
    );
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(open_file_error(error)),
    }
}

#[cfg(not(unix))]
fn remove_relative_directory_tree_checked(
    root: &Path,
    components: &[std::ffi::OsString],
) -> Result<bool, HttpResponse> {
    let mut path = root.to_path_buf();
    for component in components {
        path.push(component);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(open_file_error(error)),
        };
        if metadata.file_type().is_symlink() {
            return Err(HttpResponse::BadRequest().body("Unsafe project directory path"));
        }
    }
    std::fs::remove_dir_all(path)
        .map(|()| true)
        .map_err(open_file_error)
}

#[cfg(not(unix))]
impl ProjectFileStager {
    fn new_checked(
        root: &Path,
        components: &[std::ffi::OsString],
        replace: bool,
    ) -> Result<Self, HttpResponse> {
        let mut parent = root.to_path_buf();
        for component in &components[..components.len() - 1] {
            parent.push(component);
            let metadata = std::fs::symlink_metadata(&parent).map_err(open_file_error)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(HttpResponse::BadRequest().body("Unsafe project asset path"));
            }
        }
        let final_path = parent.join(components.last().expect("non-empty component list"));
        let temporary_path = parent.join(format!(
            ".trueshot-write-{}.part",
            uuid::Uuid::new_v4().as_simple()
        ));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&temporary_path)
            .map_err(open_file_error)?;
        Ok(Self {
            file,
            replace,
            committed: false,
            temporary_path,
            final_path,
        })
    }

    fn commit_checked(&mut self) -> Result<(), HttpResponse> {
        if !self.replace && self.final_path.exists() {
            return Err(HttpResponse::Conflict().body("Project asset already exists"));
        }
        std::fs::rename(&self.temporary_path, &self.final_path).map_err(open_file_error)
    }
}

fn open_file_error(error: std::io::Error) -> HttpResponse {
    if error.kind() == std::io::ErrorKind::NotFound {
        return HttpResponse::NotFound().body("Project asset not found");
    }
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        return HttpResponse::Conflict().body("Project asset already exists");
    }
    #[cfg(unix)]
    if matches!(error.raw_os_error(), Some(code) if code == libc::ELOOP || code == libc::ENOTDIR) {
        return HttpResponse::BadRequest().body("Unsafe project asset path");
    }
    tracing::warn!("Failed to open rooted project asset: {error}");
    HttpResponse::InternalServerError().body("Failed to open project asset")
}

pub fn project_size_bytes(root: &Path, id: &str) -> Result<u64, HttpResponse> {
    ensure_project_id(id)?;
    let root = canonicalize_root(root)?;
    let components = [std::ffi::OsString::from(id)];
    let (entries, truncated) =
        list_project_files_from_components(root, &components, 128, 1_000_000)?;
    if truncated {
        return Err(HttpResponse::PayloadTooLarge()
            .body("Project contains too many entries for a safe quota decision"));
    }
    entries.iter().try_fold(0_u64, |total, entry| {
        total
            .checked_add(entry.bytes)
            .ok_or_else(|| HttpResponse::PayloadTooLarge().body("Project size overflow"))
    })
}

#[derive(Debug, Clone)]
pub struct ProjectFileEntry {
    pub relative_path: PathBuf,
    pub bytes: u64,
    pub modified_at_unix: Option<i64>,
}

pub fn list_project_scope_files(
    root: &Path,
    id: &str,
    scope: &str,
    max_depth: usize,
    max_entries: usize,
) -> Result<(Vec<ProjectFileEntry>, bool), HttpResponse> {
    ensure_filename(scope)?;
    if max_depth == 0 || max_entries == 0 {
        return Ok((Vec::new(), false));
    }
    let candidate = root.join(id).join(scope);
    let (root, components) = project_components(root, id, &candidate)?;
    list_project_files_from_components(root, &components, max_depth, max_entries)
}

fn list_project_files_from_components(
    root: PathBuf,
    components: &[std::ffi::OsString],
    max_depth: usize,
    max_entries: usize,
) -> Result<(Vec<ProjectFileEntry>, bool), HttpResponse> {
    #[cfg(unix)]
    {
        let directory = open_relative_directory_nofollow(&root, components)?;
        let mut files = Vec::new();
        let mut visited = 0usize;
        let mut truncated = false;
        walk_relative_directory_nofollow(
            &directory,
            Path::new(""),
            1,
            max_depth,
            max_entries,
            &mut visited,
            &mut truncated,
            &mut files,
        )?;
        Ok((files, truncated))
    }
    #[cfg(not(unix))]
    {
        let directory = components
            .iter()
            .fold(root, |path, component| path.join(component));
        let mut files = Vec::new();
        let mut visited = 0usize;
        let mut truncated = false;
        walk_relative_directory_checked(
            &directory,
            &directory,
            1,
            max_depth,
            max_entries,
            &mut visited,
            &mut truncated,
            &mut files,
        )?;
        Ok((files, truncated))
    }
}

#[cfg(unix)]
fn open_relative_directory_nofollow(
    root: &Path,
    components: &[std::ffi::OsString],
) -> Result<File, HttpResponse> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;

    let mut directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)
        .map_err(open_file_error)?;
    for component in components {
        let name = CString::new(component.as_os_str().as_bytes())
            .map_err(|_| HttpResponse::BadRequest().body("Invalid project directory path"))?;
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            )
        };
        if descriptor < 0 {
            return Err(open_file_error(std::io::Error::last_os_error()));
        }
        directory = unsafe { File::from_raw_fd(descriptor) };
    }
    Ok(directory)
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
fn walk_relative_directory_nofollow(
    directory: &File,
    prefix: &Path,
    depth: usize,
    max_depth: usize,
    max_entries: usize,
    visited: &mut usize,
    truncated: &mut bool,
    files: &mut Vec<ProjectFileEntry>,
) -> Result<(), HttpResponse> {
    use std::ffi::{CStr, CString};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::MetadataExt;

    struct DirectoryStream(*mut libc::DIR);
    impl Drop for DirectoryStream {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }

    let duplicate = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(open_file_error(std::io::Error::last_os_error()));
    }
    let stream = unsafe { libc::fdopendir(duplicate) };
    if stream.is_null() {
        unsafe {
            libc::close(duplicate);
        }
        return Err(open_file_error(std::io::Error::last_os_error()));
    }
    let stream = DirectoryStream(stream);

    loop {
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        if *visited >= max_entries {
            *truncated = true;
            break;
        }
        *visited = visited.saturating_add(1);

        let name_c = CString::new(name)
            .map_err(|_| HttpResponse::BadRequest().body("Invalid project entry name"))?;
        let name_os = std::ffi::OsString::from_vec(name.to_vec());
        let relative = prefix.join(&name_os);

        let directory_descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name_c.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
            )
        };
        if directory_descriptor >= 0 {
            let child = unsafe { File::from_raw_fd(directory_descriptor) };
            if depth < max_depth {
                walk_relative_directory_nofollow(
                    &child,
                    &relative,
                    depth + 1,
                    max_depth,
                    max_entries,
                    visited,
                    truncated,
                    files,
                )?;
            }
            if *truncated {
                break;
            }
            continue;
        }

        let file_descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name_c.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if file_descriptor < 0 {
            let error = std::io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(code)
                    if code == libc::ENOENT
                        || code == libc::ELOOP
                        || code == libc::ENOTDIR
                        || code == libc::EACCES
            ) {
                continue;
            }
            return Err(open_file_error(error));
        }
        let file = unsafe { File::from_raw_fd(file_descriptor) };
        let metadata = file.metadata().map_err(open_file_error)?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            continue;
        }
        let modified_at_unix = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|duration| i64::try_from(duration.as_secs()).ok());
        files.push(ProjectFileEntry {
            relative_path: relative,
            bytes: metadata.len(),
            modified_at_unix,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::too_many_arguments)]
fn walk_relative_directory_checked(
    root: &Path,
    directory: &Path,
    depth: usize,
    max_depth: usize,
    max_entries: usize,
    visited: &mut usize,
    truncated: &mut bool,
    files: &mut Vec<ProjectFileEntry>,
) -> Result<(), HttpResponse> {
    for entry in std::fs::read_dir(directory).map_err(open_file_error)? {
        if *visited >= max_entries {
            *truncated = true;
            break;
        }
        *visited = visited.saturating_add(1);
        let entry = entry.map_err(open_file_error)?;
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(open_file_error)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if depth < max_depth {
                walk_relative_directory_checked(
                    root,
                    &entry.path(),
                    depth + 1,
                    max_depth,
                    max_entries,
                    visited,
                    truncated,
                    files,
                )?;
            }
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let relative_path = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| HttpResponse::BadRequest().body("Invalid project inventory path"))?
            .to_path_buf();
        let modified_at_unix = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|duration| i64::try_from(duration.as_secs()).ok());
        files.push(ProjectFileEntry {
            relative_path,
            bytes: metadata.len(),
            modified_at_unix,
        });
    }
    Ok(())
}

pub fn available_space_bytes(path: &Path) -> Option<u64> {
    let mut disks = Disks::new_with_refreshed_list();
    disks.refresh(false);

    let mut candidate = path.to_path_buf();
    while !candidate.exists() {
        if !candidate.pop() {
            break;
        }
    }

    let mut best: Option<(usize, u64)> = None;
    for disk in disks.list() {
        let mount = disk.mount_point();
        if candidate.starts_with(mount) {
            let len = mount.as_os_str().len();
            if best.map_or(true, |(best_len, _)| len > best_len) {
                best = Some((len, disk.available_space()));
            }
        }
    }

    best.map(|(_, space)| space)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn rooted_open_reads_only_regular_single_link_files() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let output = project.join("output");
        std::fs::create_dir_all(&output).unwrap();
        let asset = output.join("asset.bin");
        std::fs::write(&asset, b"measured").unwrap();

        let mut file = open_project_file_read(temp.path(), "project", &asset).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"measured");
    }

    #[test]
    fn project_creation_is_exclusive_and_private() {
        let temp = tempfile::tempdir().unwrap();
        let created = create_project_directory(temp.path(), "project").unwrap();
        assert!(created.is_dir());
        assert_eq!(
            create_project_directory(temp.path(), "project")
                .unwrap_err()
                .status(),
            actix_web::http::StatusCode::CONFLICT
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(created).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rooted_open_rejects_final_and_intermediate_symlinks_and_hard_links() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let output = project.join("output");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.bin");
        std::fs::write(&secret, b"secret").unwrap();

        let final_link = output.join("final-link.bin");
        symlink(&secret, &final_link).unwrap();
        assert!(open_project_file_read(temp.path(), "project", &final_link).is_err());

        let intermediate = output.join("redirect");
        symlink(&outside, &intermediate).unwrap();
        assert!(
            open_project_file_read(temp.path(), "project", &intermediate.join("secret.bin"))
                .is_err()
        );

        let hard_link = output.join("hard-link.bin");
        std::fs::hard_link(&secret, &hard_link).unwrap();
        assert!(open_project_file_read(temp.path(), "project", &hard_link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rooted_inventory_omits_links_and_never_traverses_redirected_directories() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let output = project.join("output");
        let nested = output.join("nested");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(output.join("safe.bin"), b"safe").unwrap();
        std::fs::write(nested.join("nested.bin"), b"nested").unwrap();
        let secret = outside.join("secret.bin");
        std::fs::write(&secret, b"outside-secret").unwrap();
        symlink(&secret, output.join("final-link.bin")).unwrap();
        symlink(&outside, output.join("redirect")).unwrap();
        std::fs::hard_link(&secret, output.join("hard-link.bin")).unwrap();

        let (entries, truncated) =
            list_project_scope_files(temp.path(), "project", "output", 8, 100).unwrap();
        assert!(!truncated);
        let paths = entries
            .iter()
            .map(|entry| entry.relative_path.to_string_lossy().replace('\\', "/"))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            paths,
            std::collections::BTreeSet::from([
                "nested/nested.bin".to_string(),
                "safe.bin".to_string()
            ])
        );
        assert_eq!(entries.iter().map(|entry| entry.bytes).sum::<u64>(), 10);
        assert_eq!(project_size_bytes(temp.path(), "project").unwrap(), 10);

        let (bounded, truncated) =
            list_project_scope_files(temp.path(), "project", "output", 8, 1).unwrap();
        assert!(truncated);
        assert!(bounded.len() <= 1);
    }

    #[cfg(unix)]
    #[test]
    fn rooted_descriptor_remains_bound_after_clear_and_encrypted_path_swaps() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let output = project.join("output");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let clear_path = output.join("asset.bin");
        let clear_original = output.join("asset.original");
        let clear_attacker = outside.join("attacker.bin");
        std::fs::write(&clear_path, b"measured-clear").unwrap();
        std::fs::write(&clear_attacker, b"attacker-clear").unwrap();
        let mut clear = open_project_file_read(temp.path(), "project", &clear_path).unwrap();
        std::fs::rename(&clear_path, &clear_original).unwrap();
        symlink(&clear_attacker, &clear_path).unwrap();
        let mut clear_bytes = Vec::new();
        clear.read_to_end(&mut clear_bytes).unwrap();
        assert_eq!(clear_bytes, b"measured-clear");

        let key = [17u8; 32];
        let encrypted_path = output.join("asset.raw.enc");
        let encrypted_original = output.join("asset.raw.original.enc");
        let encrypted_attacker = outside.join("attacker.raw.enc");
        trueshot_storage::encrypted::encrypt_bytes(
            &encrypted_path,
            &key,
            b"measured-encrypted",
            64 * 1024,
        )
        .unwrap();
        trueshot_storage::encrypted::encrypt_bytes(
            &encrypted_attacker,
            &key,
            b"attacker-encrypted",
            64 * 1024,
        )
        .unwrap();
        let encrypted = open_project_file_read(temp.path(), "project", &encrypted_path).unwrap();
        std::fs::rename(&encrypted_path, &encrypted_original).unwrap();
        symlink(&encrypted_attacker, &encrypted_path).unwrap();
        let mut encrypted =
            trueshot_storage::encrypted::SeekableEncryptedFile::from_file(encrypted, &key).unwrap();
        let mut encrypted_bytes = Vec::new();
        encrypted.read_to_end(&mut encrypted_bytes).unwrap();
        assert_eq!(encrypted_bytes, b"measured-encrypted");
    }

    #[cfg(unix)]
    #[test]
    fn rooted_atomic_write_replaces_a_final_symlink_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let outside_file = outside.join("project.json");
        let project_file = project.join("project.json");
        std::fs::write(&outside_file, b"outside-secret").unwrap();
        symlink(&outside_file, &project_file).unwrap();

        write_project_file_atomic(temp.path(), "project", &project_file, b"project-metadata")
            .unwrap();

        assert_eq!(std::fs::read(&outside_file).unwrap(), b"outside-secret");
        assert_eq!(std::fs::read(&project_file).unwrap(), b"project-metadata");
        assert!(!std::fs::symlink_metadata(&project_file)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn rooted_directory_creation_and_staging_reject_symlinked_parents() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let safe_dir = project.join("output").join("annotations");
        ensure_project_directory(temp.path(), "project", &safe_dir).unwrap();
        assert!(safe_dir.is_dir());

        let redirected = project.join("redirected");
        symlink(&outside, &redirected).unwrap();
        let escaped = redirected.join("asset.bin");
        assert!(stage_project_file(temp.path(), "project", &escaped, true).is_err());
        assert!(!outside.join("asset.bin").exists());
    }

    #[test]
    fn rooted_exclusive_stage_never_replaces_an_existing_target() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let target = project.join("upload.ply");
        std::fs::write(&target, b"existing").unwrap();

        let mut staged = stage_project_file(temp.path(), "project", &target, false).unwrap();
        staged.file_mut().write_all(b"attacker").unwrap();
        let response = staged.commit().unwrap_err();

        assert_eq!(response.status(), actix_web::http::StatusCode::CONFLICT);
        assert_eq!(std::fs::read(&target).unwrap(), b"existing");
    }

    #[cfg(unix)]
    #[test]
    fn rooted_remove_unlinks_the_entry_without_following_it() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let outside = temp.path().join("outside.bin");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        let target = project.join("link.bin");
        symlink(&outside, &target).unwrap();

        assert!(remove_project_file_if_exists(temp.path(), "project", &target).unwrap());
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn rooted_tree_removal_never_follows_nested_or_final_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let raw = project.join("raw");
        let nested = raw.join("nested");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(nested.join("capture.nef"), b"raw").unwrap();
        std::fs::write(outside.join("secret.nef"), b"outside").unwrap();
        symlink(outside.join("secret.nef"), raw.join("final.nef")).unwrap();
        symlink(&outside, raw.join("redirect")).unwrap();

        assert!(remove_project_directory_tree(temp.path(), "project", &raw).unwrap());
        assert!(!raw.exists());
        assert_eq!(
            std::fs::read(outside.join("secret.nef")).unwrap(),
            b"outside"
        );

        symlink(&outside, &raw).unwrap();
        assert!(remove_project_directory_tree(temp.path(), "project", &raw).unwrap());
        assert!(std::fs::symlink_metadata(&raw).is_err());
        assert_eq!(
            std::fs::read(outside.join("secret.nef")).unwrap(),
            b"outside"
        );
    }
}
