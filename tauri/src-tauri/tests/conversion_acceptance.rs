//! Run explicitly with X3FUSE_TEST_MERRILL and X3FUSE_TEST_QUATTRO absolute paths.
//! Unlike the historical integration tests, an explicit run fails when fixtures are missing.
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use x3fuse_app::{
    conversion::{process_options, publish},
    metadata::ExifTool,
    model::{BatchSettings, DngLook, OutputFormat},
    storage::Logs,
};

fn fixture(key: &str) -> PathBuf {
    let path = PathBuf::from(
        std::env::var(key).unwrap_or_else(|_| panic!("Set {key} to an X3F corpus file")),
    );
    assert!(
        path.is_absolute() && path.is_file(),
        "Missing fixture: {}",
        path.display()
    );
    path
}

#[test]
#[ignore = "Requires X3FUSE_TEST_MERRILL and prepared resources"]
fn dng_looks_survive_metadata_copy_and_preserve_calibration() {
    let temporary = tempfile::tempdir().unwrap();
    let input = temporary.path().join("写真 source.X3F");
    std::fs::copy(fixture("X3FUSE_TEST_MERRILL"), &input).unwrap();
    let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
    let profile = resources.join("profiles/merrill_spp_1_0.dcp");
    let exif = ExifTool::new(
        resources.join("exiftool"),
        Arc::new(Logs::new(temporary.path().join("logs"))),
    );
    let runtime = tauri::async_runtime::handle();
    let table = |path: &std::path::Path| {
        runtime
            .block_on(exif.run(
                // ExifTool skips large arrays unless minor-error suppression is enabled.
                vec![
                    "-m".into(),
                    "-b".into(),
                    "-ProfileLookTableData".into(),
                    path.into(),
                ],
                None,
            ))
            .unwrap()
    };
    let expected = table(&profile);
    assert!(!expected.is_empty());
    let mut baseline = None;
    for (index, look) in [
        DngLook::None,
        DngLook::MerrillSpp10,
        DngLook::Custom(profile),
    ]
    .into_iter()
    .enumerate()
    {
        let settings = BatchSettings {
            dng_look: look,
            denoise_intensity: 0,
            ..Default::default()
        };
        let output = temporary.path().join(format!("look-{index}.dng"));
        x3f_core::convert_file(
            &input,
            &output,
            x3f_core::OutputFormat::Dng,
            &process_options(&settings, resources.clone()),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        runtime
            .block_on(exif.copy_tags(&input, &output, &AtomicBool::new(false)))
            .unwrap();
        let actual = table(&output);
        if index == 0 {
            assert!(actual.is_empty());
        } else {
            assert_eq!(actual, expected);
        }
        let mut args: Vec<std::ffi::OsString> = [
            "-json",
            "-UniqueCameraModel",
            "-ColorMatrix1",
            "-ColorMatrix2",
            "-ForwardMatrix1",
            "-ForwardMatrix2",
            "-AsShotNeutral",
            "-BlackLevel",
            "-WhiteLevel",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.push(output.into());
        let bytes = runtime.block_on(exif.run(args, None)).unwrap();
        let mut tags: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
        let tags = tags[0].as_object_mut().unwrap();
        tags.remove("SourceFile");
        assert!(tags.contains_key("ColorMatrix1"));
        assert!(tags.contains_key("UniqueCameraModel"));
        if let Some(baseline) = &baseline {
            assert_eq!(tags, baseline);
        } else {
            baseline = Some(tags.clone());
        }
    }
}

#[test]
#[ignore = "Requires X3FUSE_TEST_MERRILL, X3FUSE_TEST_POST_PROCESSING_COMMAND and prepared resources"]
fn post_processing_uses_the_published_dng() {
    use sha2::{Digest, Sha256};
    let command = std::env::var("X3FUSE_TEST_POST_PROCESSING_COMMAND")
        .expect("Set a trusted post-processing command explicitly");
    assert!(!command.trim().is_empty());
    let temporary = tempfile::tempdir().unwrap();
    let input = temporary.path().join("写真 source.X3F");
    std::fs::copy(fixture("X3FUSE_TEST_MERRILL"), &input).unwrap();
    let source_hash = Sha256::digest(std::fs::read(&input).unwrap());
    let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
    let exif = ExifTool::new(
        resources.join("exiftool"),
        Arc::new(Logs::new(temporary.path().join("logs"))),
    );
    let settings = BatchSettings {
        dng_look: DngLook::MerrillSpp10,
        dng_post_processing_command: command,
        denoise_intensity: 0,
        ..Default::default()
    };
    let staging = temporary.path().join("staged.dng");
    let output = settings.output_path(&input).unwrap();
    let cancel = AtomicBool::new(false);
    x3f_core::convert_file(
        &input,
        &staging,
        x3f_core::OutputFormat::Dng,
        &process_options(&settings, resources.clone()),
        &cancel,
        |_| {},
    )
    .unwrap();
    let runtime = tauri::async_runtime::handle();
    runtime
        .block_on(exif.copy_tags(&input, &staging, &cancel))
        .unwrap();
    publish(&staging, &output, false).unwrap();
    assert!(!staging.exists());
    runtime
        .block_on(x3fuse_app::post_processing::apply(
            &input, &output, &settings, &resources, &exif, &cancel,
        ))
        .unwrap();
    assert!(std::fs::metadata(&output).unwrap().len() > 0);
    assert_eq!(Sha256::digest(std::fs::read(&input).unwrap()), source_hash);
    println!(
        "{}",
        std::fs::read_to_string(exif.logs.dir.join("conversion.log")).unwrap()
    );
}

#[test]
#[ignore = "Requires explicitly supplied Merrill and Quattro corpus files plus prepared resources"]
fn real_conversion_metadata_previews_and_cancelled_publication() {
    let samples = [
        fixture("X3FUSE_TEST_MERRILL"),
        fixture("X3FUSE_TEST_QUATTRO"),
    ];
    let temporary = tempfile::tempdir().unwrap();
    let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
    let exif = ExifTool::new(
        resources.join("exiftool"),
        Arc::new(Logs::new(temporary.path().join("logs"))),
    );
    let runtime = tauri::async_runtime::handle();
    runtime.block_on(exif.check()).unwrap();
    for (camera, original) in samples.iter().enumerate() {
        // Exercise spaces, percent signs and Unicode at the real native file boundary.
        let input = temporary.path().join(format!("写真 {camera} 100%.X3F"));
        std::fs::copy(original, &input).unwrap();
        assert!(!runtime.block_on(exif.full(&input)).unwrap().is_empty());
        assert!(runtime
            .block_on(exif.preview(&input, false))
            .unwrap()
            .is_some());
        assert!(runtime
            .block_on(exif.preview(&input, true))
            .unwrap()
            .is_some());
        for (format, core_format) in [
            (OutputFormat::Dng, x3f_core::OutputFormat::Dng),
            (OutputFormat::Tiff, x3f_core::OutputFormat::Tiff),
            (OutputFormat::Jpeg, x3f_core::OutputFormat::Jpeg),
        ] {
            let settings = BatchSettings {
                output_format: format,
                denoise_intensity: 0,
                output_directory: Some(temporary.path().to_owned()),
                ..Default::default()
            };
            let options = process_options(&settings, resources.clone());
            let staging = tempfile::tempdir_in(temporary.path()).unwrap();
            let output = staging
                .path()
                .join(format!("output.{}", format.extension()));
            let mut stages = vec![];
            x3f_core::convert_file(
                &input,
                &output,
                core_format,
                &options,
                &AtomicBool::new(false),
                |s| stages.push(s),
            )
            .unwrap();
            if format == OutputFormat::Dng {
                runtime
                    .block_on(exif.copy_tags(&input, &output, &AtomicBool::new(false)))
                    .unwrap();
            }
            let bytes = std::fs::read(&output).unwrap();
            assert!(bytes.len() > 100);
            if format == OutputFormat::Jpeg {
                assert!(bytes.starts_with(&[0xff, 0xd8]));
            } else {
                assert!(bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*"));
            }
            let final_output = settings.output_path(&input).unwrap();
            publish(&output, &final_output, false).unwrap();
            assert!(final_output.is_file());
            assert!(!output.exists());
            if format == OutputFormat::Dng {
                let tags = |path: &std::path::Path| {
                    let bytes = runtime
                        .block_on(exif.run(
                            vec![
                                "-json".into(),
                                "-Model".into(),
                                "-DateTimeOriginal".into(),
                                path.into(),
                            ],
                            None,
                        ))
                        .unwrap();
                    serde_json::from_slice::<Vec<serde_json::Value>>(&bytes)
                        .unwrap()
                        .remove(0)
                };
                let source = tags(&input);
                let output = tags(&final_output);
                assert_eq!(source["Model"], output["Model"]);
                assert_eq!(source["DateTimeOriginal"], output["DateTimeOriginal"]);
            }
            assert!(!stages.is_empty());
        }
        let target = temporary.path().join(format!("cancelled-{camera}.dng"));
        let cancel = AtomicBool::new(false);
        let result = x3f_core::convert_file(
            &input,
            &target,
            x3f_core::OutputFormat::Dng,
            &x3f_core::ProcessOptions::default(),
            &cancel,
            |stage| {
                if matches!(stage, x3f_core::ConversionStage::Process) {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert!(matches!(result, Err(x3f_core::Error::Cancelled)));
        assert!(!target.exists());
    }
}

#[test]
#[ignore = "Requires explicitly supplied Merrill and Quattro corpus files"]
fn full_denoise_conversion_finishes_within_deadline() {
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    let deadline = Duration::from_secs(60);
    let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
    for key in ["X3FUSE_TEST_MERRILL", "X3FUSE_TEST_QUATTRO"] {
        let input = fixture(key);
        for (format, core_format) in [
            (OutputFormat::Dng, x3f_core::OutputFormat::Dng),
            (OutputFormat::Tiff, x3f_core::OutputFormat::Tiff),
        ] {
            let settings = BatchSettings {
                output_format: format,
                compress: true,
                denoise_intensity: 10,
                dng_highlight_recovery: true,
                ..Default::default()
            };
            let options = process_options(&settings, resources.clone());
            let temporary = tempfile::tempdir().unwrap();
            let output = temporary
                .path()
                .join(format!("full-denoise.{}", format.extension()));
            let cancel = AtomicBool::new(false);
            let mut reached_write = false;
            let started = Instant::now();
            let result = std::thread::scope(|scope| {
                let (done, completion) = mpsc::channel::<()>();
                let cancel = &cancel;
                scope.spawn(move || {
                    if matches!(
                        completion.recv_timeout(deadline),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ) {
                        cancel.store(true, Ordering::Relaxed);
                    }
                });
                let result = x3f_core::convert_file(
                    &input,
                    &output,
                    core_format,
                    &options,
                    cancel,
                    |stage| reached_write |= stage == x3f_core::ConversionStage::Write,
                );
                let _ = done.send(());
                result
            });
            let elapsed = started.elapsed();
            eprintln!(
                "Full-denoise {format:?} conversion of {}: {elapsed:?}",
                input.display()
            );
            if result.is_err() {
                assert!(!output.exists(), "Failed conversion left a partial output");
            }
            assert!(
                result.is_ok(),
                "{key} {format:?} failed after {elapsed:?}: {result:?}"
            );
            assert!(elapsed < deadline && !cancel.load(Ordering::Relaxed));
            assert!(reached_write);
            assert!(std::fs::metadata(&output).unwrap().len() > 100);
        }
    }
}
