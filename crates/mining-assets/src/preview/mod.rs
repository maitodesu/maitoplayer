use contracts::TimestampUs;

use crate::workflow::AssetBundle;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewMetadata {
    pub audio_media_name: String,
    pub image_media_name: String,
    pub duration_us: TimestampUs,
    pub frame_us: TimestampUs,
}

impl From<&AssetBundle> for PreviewMetadata {
    fn from(bundle: &AssetBundle) -> Self {
        Self {
            audio_media_name: bundle.audio.media_name.clone(),
            image_media_name: bundle.image.media_name.clone(),
            duration_us: bundle.audio_end_us - bundle.audio_start_us,
            frame_us: bundle.frame_us,
        }
    }
}
