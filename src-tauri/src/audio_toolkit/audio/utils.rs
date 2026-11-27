use anyhow::Result;
use hound::{WavReader, WavSpec, WavWriter};
use log::debug;
use std::path::Path;

/// Read a WAV file and return normalised f32 samples.
pub fn read_wav_samples<P: AsRef<Path>>(file_path: P) -> Result<Vec<f32>> {
    let reader = WavReader::open(file_path.as_ref())?;
    let samples = reader
        .into_samples::<i16>()
        .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
        .collect::<Result<Vec<f32>, _>>()?;
    Ok(samples)
}

/// Verify a WAV file by reading it back and checking the sample count.
pub fn verify_wav_file<P: AsRef<Path>>(file_path: P, expected_samples: usize) -> Result<()> {
    let reader = WavReader::open(file_path.as_ref())?;
    let actual_samples = reader.len() as usize;
    if actual_samples != expected_samples {
        anyhow::bail!(
            "WAV sample count mismatch: expected {}, got {}",
            expected_samples,
            actual_samples
        );
    }
    Ok(())
}

/// Save audio samples as a WAV file
pub fn save_wav_file<P: AsRef<Path>>(file_path: P, samples: &[f32]) -> Result<()> {
    let spec = WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = WavWriter::create(file_path.as_ref(), spec)?;

    // Convert f32 samples to i16 for WAV
    for sample in samples {
        let sample_i16 = (sample * i16::MAX as f32) as i16;
        writer.write_sample(sample_i16)?;
    }

    writer.finalize()?;
    debug!("Saved WAV file: {:?}", file_path.as_ref());
    Ok(())
}

/// Validates that a local audio file path is a regular non-empty file `<= 512 MiB`
/// with no symlink or Windows reparse-point components anywhere in its path chain.
pub fn validate_bounded_local_audio_file(path: &Path) -> std::result::Result<u64, String> {
    const MAX_AUDIO_FILE_BYTES: u64 = 512 * 1024 * 1024;

    let check_not_symlink_or_reparse =
        |candidate: &Path| -> std::result::Result<std::fs::Metadata, String> {
            let meta = std::fs::symlink_metadata(candidate)
                .map_err(|e| format!("cannot inspect audio path {}: {e}", candidate.display()))?;
            if meta.file_type().is_symlink() {
                return Err(format!(
                    "refusing symlink in audio path: {}",
                    candidate.display()
                ));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
                if (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
                    return Err(format!(
                        "refusing Windows reparse point in audio path: {}",
                        candidate.display()
                    ));
                }
            }
            Ok(meta)
        };

    let leaf_meta = check_not_symlink_or_reparse(path)?;
    if !leaf_meta.file_type().is_file() {
        return Err(format!(
            "audio path is not a regular file: {}",
            path.display()
        ));
    }

    for ancestor in path.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() || ancestor.parent().is_none() {
            break;
        }
        let _ = check_not_symlink_or_reparse(ancestor)?;
    }

    let len = leaf_meta.len();
    if len == 0 {
        return Err("audio file is empty".into());
    }
    if len > MAX_AUDIO_FILE_BYTES {
        return Err(format!(
            "audio file size {len} exceeds 512 MiB safety limit"
        ));
    }
    Ok(len)
}

#[cfg(test)]
mod bounded_audio_tests {
    use super::validate_bounded_local_audio_file;
    use tempfile::TempDir;

    #[test]
    fn accepts_regular_file_and_rejects_empty_oversized_directory_and_reparse_ancestors() {
        let dir = TempDir::new().unwrap();

        // Directory rejected
        assert!(validate_bounded_local_audio_file(dir.path()).is_err());

        // Empty file rejected
        let empty = dir.path().join("empty.wav");
        std::fs::write(&empty, b"").unwrap();
        assert!(validate_bounded_local_audio_file(&empty).is_err());

        // Regular file accepted
        let ok_file = dir.path().join("ok.wav");
        std::fs::write(&ok_file, b"RIFF----WAVEfmt ").unwrap();
        assert_eq!(validate_bounded_local_audio_file(&ok_file), Ok(16));

        // > 512 MiB sparse file rejected
        let huge = dir.path().join("huge.wav");
        let f = std::fs::File::create(&huge).unwrap();
        f.set_len(512 * 1024 * 1024 + 1).unwrap();
        drop(f);
        assert!(validate_bounded_local_audio_file(&huge).is_err());

        // Windows directory junction ancestor rejected
        #[cfg(windows)]
        {
            let real_sub = dir.path().join("real_sub");
            std::fs::create_dir(&real_sub).unwrap();
            let nested_wav = real_sub.join("clip.wav");
            std::fs::write(&nested_wav, b"RIFF----WAVEfmt ").unwrap();

            let junc = dir.path().join("junc_sub");
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&junc)
                .arg(&real_sub)
                .output()
                .unwrap();
            assert!(status.status.success(), "mklink /J failed");

            let via_junc = junc.join("clip.wav");
            let err = validate_bounded_local_audio_file(&via_junc).unwrap_err();
            assert!(
                err.contains("symlink") || err.contains("reparse point"),
                "unexpected error: {err}"
            );
            let _ = std::fs::remove_dir(&junc);
        }

        #[cfg(unix)]
        {
            let real_sub = dir.path().join("real_sub");
            std::fs::create_dir(&real_sub).unwrap();
            let nested_wav = real_sub.join("clip.wav");
            std::fs::write(&nested_wav, b"RIFF----WAVEfmt ").unwrap();
            let sym = dir.path().join("sym_sub");
            std::os::unix::fs::symlink(&real_sub, &sym).unwrap();
            let via_sym = sym.join("clip.wav");
            assert!(validate_bounded_local_audio_file(&via_sym).is_err());
        }
    }
}
