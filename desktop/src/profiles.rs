use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayProfile {
    pub id: String,
    pub name: String,
    pub origin: Url,
}

impl GatewayProfile {
    pub fn endpoint(&self, segments: &[&str]) -> Url {
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .expect("a Gateway origin is an HTTPS URL")
            .clear()
            .extend(segments.iter().copied());
        url
    }

    pub fn project_stream(&self, project: &str) -> Url {
        let mut url = self.endpoint(&["api", "projects", project, "chat", "ws"]);
        url.set_scheme("wss")
            .expect("HTTPS and WSS share URL syntax");
        url
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayProfiles {
    pub profiles: Vec<GatewayProfile>,
    active_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("Enter a profile name and an HTTPS or WSS Gateway origin.")]
    InvalidProfile,
    #[error("The Gateway profile does not exist.")]
    UnknownProfile,
    #[error("Desktop configuration could not be read or written: {0}")]
    Io(#[from] io::Error),
    #[error("Desktop configuration is invalid: {0}")]
    Format(#[from] serde_json::Error),
}

impl GatewayProfiles {
    pub fn add(&mut self, name: &str, origin: &str) -> Result<String, ProfileError> {
        let name = name.trim();
        let mut origin = Url::parse(origin.trim()).map_err(|_| ProfileError::InvalidProfile)?;
        if name.is_empty() || !matches!(origin.scheme(), "https" | "wss") || !is_origin(&origin) {
            return Err(ProfileError::InvalidProfile);
        }
        origin
            .set_scheme("https")
            .map_err(|_| ProfileError::InvalidProfile)?;
        let id = Uuid::new_v4().to_string();
        self.profiles.push(GatewayProfile {
            id: id.clone(),
            name: name.to_owned(),
            origin,
        });
        self.active_id = Some(id.clone());
        Ok(id)
    }

    pub fn select(&mut self, id: &str) -> Result<(), ProfileError> {
        if !self.profiles.iter().any(|profile| profile.id == id) {
            return Err(ProfileError::UnknownProfile);
        }
        self.active_id = Some(id.to_owned());
        Ok(())
    }

    pub fn active(&self) -> Option<&GatewayProfile> {
        self.profiles
            .iter()
            .find(|profile| Some(profile.id.as_str()) == self.active_id.as_deref())
    }

    pub fn load(path: &Path) -> Result<Self, ProfileError> {
        match fs::read(path) {
            Ok(bytes) => {
                let profiles = serde_json::from_slice::<Self>(&bytes)?;
                if profiles.profiles.iter().any(|profile| {
                    profile.origin.scheme() != "https" || !is_origin(&profile.origin)
                }) {
                    return Err(ProfileError::InvalidProfile);
                }
                Ok(profiles)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ProfileError> {
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temp, self)?;
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|error| error.error)?;
        Ok(())
    }
}

fn is_origin(url: &Url) -> bool {
    url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
}
