use crate::{Error, Result, blocking, io_error, paths};
use std::{
    collections::HashMap,
    fs, io,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use t3_contracts::{
    project::{Project, ProjectScript, ProjectScriptIcon},
    t3_project_file::T3ProjectFile,
};
use tokio::{sync::Mutex, time::Instant};

pub fn parse_project_file(raw: &str) -> Option<T3ProjectFile> {
    if raw.len() > 1024 * 1024 {
        return None;
    }
    let comments = regex::Regex::new(r#"("(?:[^"\\]|\\.)*")|//[^\r\n]*|/\*[\s\S]*?\*/"#).ok()?;
    let stripped = comments.replace_all(raw, |c: &regex::Captures<'_>| {
        c.get(1).map_or(" ", |s| s.as_str()).to_owned()
    });
    let commas = regex::Regex::new(r#"("(?:[^"\\]|\\.)*")|,(\s*[}\]])"#).ok()?;
    let stripped = commas.replace_all(&stripped, |c: &regex::Captures<'_>| {
        c.get(1)
            .or_else(|| c.get(2))
            .map_or("", |s| s.as_str())
            .to_owned()
    });
    let file: T3ProjectFile = serde_json::from_str(&stripped).ok()?;
    if file
        .icon_path
        .as_ref()
        .is_some_and(|p| p.0.encode_utf16().count() > 512)
        || file.scripts.as_ref().is_some_and(|s| s.len() > 50)
    {
        return None;
    }
    Some(file)
}
pub async fn load_project_file(cwd: &Path) -> Option<T3ProjectFile> {
    let root = cwd.to_owned();
    blocking(move || {
        let scope = crate::scoped::ScopedDir::open(&root)?;
        let relative = scope.resolve("t3.json", false)?;
        let file = scope.open_file(&relative)?;
        let mut raw = String::new();
        file.take(1024 * 1024 + 1)
            .read_to_string(&mut raw)
            .map_err(|e| io_error("read-project-file", &root, e))?;
        Ok(parse_project_file(&raw))
    })
    .await
    .ok()
    .flatten()
}

pub const FAVICON_CANDIDATES: &[&str] = &[
    "favicon.svg",
    "favicon.ico",
    "favicon.png",
    "public/favicon.svg",
    "public/favicon.ico",
    "public/favicon.png",
    "app/favicon.ico",
    "app/favicon.png",
    "app/icon.svg",
    "app/icon.png",
    "app/icon.ico",
    "src/favicon.ico",
    "src/favicon.svg",
    "src/app/favicon.ico",
    "src/app/icon.svg",
    "src/app/icon.png",
    "assets/icon.svg",
    "assets/icon.png",
    "assets/logo.svg",
    "assets/logo.png",
    ".idea/icon.svg",
    "Resources/AppIcon.icns",
    "AppIcon.icns",
];
const ICON_SOURCES: &[&str] = &[
    "index.html",
    "public/index.html",
    "app/routes/__root.tsx",
    "src/routes/__root.tsx",
    "app/root.tsx",
    "src/root.tsx",
    "src/index.html",
];

pub fn extract_icon_href(source: &str) -> Option<String> {
    let link = regex::Regex::new(r"(?i)<link\b[^>]*>").ok()?;
    let rel = regex::Regex::new(r#"(?i)\brel=["'](?:icon|shortcut icon)["']"#).ok()?;
    let href = regex::Regex::new(r#"(?i)\bhref=["']([^"'?]+)"#).ok()?;
    for tag in link.find_iter(source) {
        if rel.is_match(tag.as_str())
            && let Some(c) = href.captures(tag.as_str())
        {
            return Some(c[1].into());
        }
    }
    let rel = regex::Regex::new(r#"(?i)\brel\s*:\s*["'](?:icon|shortcut icon)["']"#).ok()?;
    let href = regex::Regex::new(r#"(?i)\bhref\s*:\s*["']([^"'?]+)"#).ok()?;
    for run in source.split('}') {
        if rel.is_match(run)
            && let Some(c) = href.captures(run)
        {
            return Some(c[1].into());
        }
    }
    None
}
type FaviconCache =
    HashMap<(PathBuf, Option<String>), (std::result::Result<Option<PathBuf>, PathBuf>, Instant)>;
pub struct FaviconResolver {
    pub home: PathBuf,
    cache: Mutex<FaviconCache>,
}
impl FaviconResolver {
    pub fn new(home: PathBuf) -> Self {
        Self {
            home,
            cache: Default::default(),
        }
    }
    pub async fn invalidate(&self, cwd: &Path) {
        let expanded = paths::expand_home(&cwd.to_string_lossy(), &self.home);
        let normalized =
            paths::absolute(&expanded).and_then(|p| paths::nearest_real(&p).map(|p| p.0));
        if let Ok(cwd) = normalized {
            self.cache.lock().await.retain(|(path, _), _| *path != cwd);
        }
    }
    pub async fn resolve(&self, cwd: &str, explicit: Option<&str>) -> Result<Option<PathBuf>> {
        let expanded = paths::expand_home(cwd.trim(), &self.home);
        paths::validate_host_path(&expanded)?;
        let normalized = paths::nearest_real(&paths::absolute(&expanded)?)?.0;
        let key = (normalized, explicit.map(str::to_owned));
        let cache = self.cache.lock().await;
        if let Some((lookup, expires)) = cache.get(&key)
            && Instant::now() < *expires
        {
            match lookup {
                Ok(path) if path.as_ref().is_none_or(|p| p.is_file()) => return Ok(path.clone()),
                Err(path) => return Err(Error::RootMissing(path.clone())),
                _ => {}
            }
        }
        drop(cache);
        let root = match paths::normalize_root(cwd, &self.home, false).await {
            Ok(root) => root,
            Err(Error::RootMissing(path)) => {
                let mut cache = self.cache.lock().await;
                if cache.len() >= 512
                    && let Some(oldest) = cache
                        .iter()
                        .min_by_key(|(_, (_, expires))| *expires)
                        .map(|(key, _)| key.clone())
                {
                    cache.remove(&oldest);
                }
                cache.insert(
                    key,
                    (Err(path.clone()), Instant::now() + Duration::from_secs(60)),
                );
                return Err(Error::RootMissing(path));
            }
            Err(error) => return Err(error),
        };
        let file = load_project_file(&root).await;
        let explicit = explicit.map(str::to_owned);
        let found = blocking(move || {
            let find = |relative: &str, absolute: bool| -> Result<Option<PathBuf>> {
                let candidate = if absolute && Path::new(relative).is_absolute() {
                    PathBuf::from(relative)
                } else {
                    match paths::resolve_relative(&root, relative) {
                        Ok((p, _)) => p,
                        Err(Error::OutsideRoot(_)) => return Ok(None),
                        Err(e) => return Err(e),
                    }
                };
                match fs::metadata(&candidate) {
                    Ok(m) => Ok(m.is_file().then_some(candidate)),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(io_error("stat-candidate", candidate, e)),
                }
            };
            if let Some(explicit) = explicit
                && let Some(found) = find(&explicit, true)?
            {
                return Ok(Some(found));
            }
            if let Some(icon) = file.and_then(|f| f.icon_path)
                && let Some(found) = find(&icon.0, false)?
            {
                return Ok(Some(found));
            }
            for candidate in FAVICON_CANDIDATES {
                if let Some(found) = find(candidate, false)? {
                    return Ok(Some(found));
                }
            }
            for source in ICON_SOURCES {
                let p = root.join(source);
                let scope = crate::scoped::ScopedDir::open(&root)?;
                let Ok(relative) = scope.resolve(source, false) else {
                    continue;
                };
                let Ok(file) = scope.open_file(&relative) else {
                    continue;
                };
                let mut contents = String::new();
                file.take(1024 * 1024 + 1)
                    .read_to_string(&mut contents)
                    .map_err(|e| io_error("read-source", &p, e))?;
                if contents.len() > 1024 * 1024 {
                    continue;
                }
                if let Some(href) = extract_icon_href(&contents) {
                    let clean = href.trim().strip_prefix('/').unwrap_or(href.trim());
                    for candidate in [format!("public/{clean}"), clean.into()] {
                        if let Some(found) = find(&candidate, false)? {
                            return Ok(Some(found));
                        }
                    }
                }
            }
            Ok(None)
        })
        .await?;
        let mut cache = self.cache.lock().await;
        if cache.len() >= 512
            && let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, (_, expires))| *expires)
                .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest);
        }
        let ttl = if found.is_some() { 10 * 60 } else { 60 };
        cache.insert(
            key,
            (Ok(found.clone()), Instant::now() + Duration::from_secs(ttl)),
        );
        Ok(found)
    }
}

/// A projection snapshot: persistence and command commits remain in orchestration.
pub struct ProjectFiles {
    pub home: PathBuf,
    pub favicons: Arc<FaviconResolver>,
}
impl ProjectFiles {
    pub async fn validate_root(&self, root: &str, create: bool) -> Result<PathBuf> {
        paths::normalize_root(root, &self.home, create).await
    }
    pub async fn find_by_root<'a>(
        &self,
        projects: &'a [Project],
        root: &str,
        include_deleted: bool,
    ) -> Result<Option<&'a Project>> {
        let normalized = paths::absolute(&paths::expand_home(root.trim(), &self.home))?;
        Ok(projects.iter().find(|p| {
            (include_deleted || p.deleted_at.is_none())
                && Path::new(&p.workspace_root.0) == normalized
        }))
    }
    pub async fn configuration(&self, project: &Project) -> Option<T3ProjectFile> {
        load_project_file(Path::new(&project.workspace_root.0)).await
    }
}

#[derive(Default)]
pub struct ScriptSettings {
    pub defaults: Vec<ProjectScript>,
    pub overrides: HashMap<String, Vec<ProjectScript>>,
    pub legacy_overrides: HashMap<String, Option<Vec<ProjectScript>>>,
    pub folded: bool,
}
pub fn resolve_scripts<'a>(
    settings: &'a ScriptSettings,
    project_id: &str,
    project: &'a [ProjectScript],
) -> &'a [ProjectScript] {
    if let Some(scripts) = settings.overrides.get(project_id) {
        return scripts;
    }
    if settings.folded {
        return &settings.defaults;
    }
    match settings.legacy_overrides.get(project_id) {
        Some(Some(scripts)) => scripts,
        Some(None) => &settings.defaults,
        None if !project.is_empty() => project,
        None => &settings.defaults,
    }
}
pub fn setup_script(scripts: &[ProjectScript]) -> Option<&ProjectScript> {
    scripts.iter().find(|s| s.run_on_worktree_create)
}
pub fn settle_script(scripts: &[ProjectScript]) -> Option<&ProjectScript> {
    scripts.iter().find(|s| s.run_on_settle == Some(Some(true)))
}
pub fn script_runtime_env(root: &str, worktree: Option<&str>) -> HashMap<String, String> {
    let mut env = HashMap::from([("T3CODE_PROJECT_ROOT".into(), root.into())]);
    if let Some(worktree) = worktree.filter(|s| !s.is_empty()) {
        env.insert("T3CODE_WORKTREE_PATH".into(), worktree.into());
    }
    env
}
pub fn script_terminal_input(command: &str) -> String {
    format!(" {command}\r")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionShell {
    Posix,
    Fish,
    Nushell,
    PowerShell,
}
pub fn completion_shell(shell: &str) -> Result<CompletionShell> {
    let name = shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(shell)
        .to_ascii_lowercase();
    match name.trim_end_matches(".exe") {
        "cmd" => Err(Error::Invalid(
            "Project script completion is not supported in cmd.exe. Install PowerShell and retry."
                .into(),
        )),
        "fish" => Ok(CompletionShell::Fish),
        "nu" => Ok(CompletionShell::Nushell),
        "pwsh" | "powershell" => Ok(CompletionShell::PowerShell),
        _ => Ok(CompletionShell::Posix),
    }
}
pub fn completion_sentinel(token: &str) -> Result<String> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(Error::Invalid("invalid completion token".into()));
    }
    Ok(format!("__T3_SETUP_DONE___{token}:"))
}
pub fn wrap_script(command: &str, shell: CompletionShell, token: &str) -> Result<String> {
    let sentinel = completion_sentinel(token)?;
    let body = command.replace("\r\n", "\r").replace('\n', "\r");
    Ok(match shell {
        CompletionShell::PowerShell => format!(
            "$global:LASTEXITCODE = $null; & {{\r{body}\r}}; if ($null -ne $LASTEXITCODE) {{ $__t3c = $LASTEXITCODE }} elseif ($?) {{ $__t3c = 0 }} else {{ $__t3c = 1 }}; Write-Host \"{sentinel}$__t3c\""
        ),
        CompletionShell::Fish => format!("begin\r{body}\rend; printf '\\n{sentinel}%s\\n' $status"),
        CompletionShell::Posix => format!("( {body}\r); printf '\\n{sentinel}%s\\n' \"$?\""),
        CompletionShell::Nushell => {
            let escaped: String = command
                .chars()
                .map(|c| {
                    if c == '"' || c == '\\' {
                        format!("\\{c}")
                    } else if c <= '\u{1f}' {
                        format!("\\u{{{:x}}}", c as u32)
                    } else {
                        c.to_string()
                    }
                })
                .collect();
            format!(
                "$env.LAST_EXIT_CODE = 0; let __t3f = try {{ mktemp --tmpdir-path $env.T3CODE_WORKTREE_PATH --suffix .nu .t3-setup-XXXXXXXX }} catch {{|e| print -e $e.msg; null }}; if $__t3f == null {{ $env.LAST_EXIT_CODE = 1 }} else {{ try {{ [$\"rm --permanent ($__t3f | to nuon)\" \"{escaped}\"] | str join (char newline) | save --force --raw $__t3f; ^$nu.current-exe --login --commands $\"cd $env.T3CODE_WORKTREE_PATH; source ($__t3f | to nuon)\" }} catch {{|e| if $e != null and $e.exit_code? == null {{ print -e $e.msg }}; if $env.LAST_EXIT_CODE == 0 {{ $env.LAST_EXIT_CODE = 1 }} }} }}; let __t3c = $env.LAST_EXIT_CODE; try {{ if $__t3f != null and ($__t3f | path exists) {{ rm --permanent $__t3f }} }} catch {{|e| print -e $e.msg }}; print $\"\\n{sentinel}($__t3c)\""
            )
        }
    })
}

pub struct ScriptOutput {
    sentinel: String,
    buffer: String,
    pub exit_code: Option<i32>,
    pub settled: bool,
    echoed: Vec<String>,
}
impl ScriptOutput {
    pub fn new(token: &str, wrapper: &str) -> Result<Self> {
        Ok(Self {
            sentinel: completion_sentinel(token)?,
            buffer: String::new(),
            exit_code: None,
            settled: false,
            echoed: wrapper
                .split('\r')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        })
    }
    pub fn feed(&mut self, text: &str) -> Vec<String> {
        self.buffer.push_str(text);
        let text = self.buffer.replace("\r\n", "\n").replace('\r', "\n");
        let mut lines: Vec<_> = text.split('\n').map(str::to_owned).collect();
        self.buffer = lines.pop().unwrap_or_default();
        if self.buffer.encode_utf16().count() > 4096 {
            self.buffer = self
                .buffer
                .chars()
                .rev()
                .take(4096)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
        }
        let mut output = Vec::new();
        for line in lines {
            if let Some((_, suffix)) = line.split_once(&self.sentinel) {
                let number: String = suffix
                    .chars()
                    .take_while(|c| *c == '-' || c.is_ascii_digit())
                    .collect();
                if let Ok(code) = number.parse() {
                    self.exit_code = Some(code);
                    self.settled = true;
                    continue;
                }
            }
            let clean = strip_terminal_control(&line);
            let clean = clean.trim_end();
            if clean.is_empty()
                || clean.contains(&self.sentinel)
                || self.echoed.iter().any(|s| clean.ends_with(s))
            {
                continue;
            }
            output.push(clean.chars().take(400).collect());
        }
        output
    }
    pub fn closed(&mut self) {
        if !self.settled {
            self.exit_code = None;
            self.settled = true;
        }
    }
}
pub fn strip_terminal_control(text: &str) -> String {
    let Ok(re) = regex::Regex::new(
        "\x1b\\[[0-9;?]*[ -/]*[@-~]|\x1b\\][^\x07\x1b]*(?:\x07|\x1b\\\\)|\x1b[()][A-Za-z0-9]|\x1b[=>]",
    ) else {
        return text.into();
    };
    re.replace_all(text, "")
        .chars()
        .filter(|c| !matches!(*c as u32, 0..=8|11|12|14..=31|127))
        .collect()
}

pub fn scripts_from_file(
    file: &T3ProjectFile,
    ids: impl IntoIterator<Item = String>,
) -> Vec<ProjectScript> {
    file.scripts
        .iter()
        .flatten()
        .zip(ids)
        .map(|(s, id)| ProjectScript {
            id: t3_contracts::base_schemas::TrimmedNonEmptyString(id),
            name: s.name.clone(),
            command: s.command.clone(),
            icon: s.icon.clone().unwrap_or(ProjectScriptIcon::Play),
            run_on_worktree_create: s.run_on_worktree_create.unwrap_or(false),
            run_on_settle: s.run_on_settle.map(Some),
            stop_on_settle: s.stop_on_settle.map(Some),
            r#async: s.r#async.map(Some),
            preview_url: s.preview_url.clone().map(Some),
            auto_open_preview: s.auto_open_preview.map(Some),
        })
        .collect()
}
