use std::ffi::OsString;
use std::fs::{self, read_to_string, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use image::{DynamicImage, GenericImageView, Rgba};
use img_hash::{HasherConfig, ImageHash};
use log::info;
use rayon::prelude::*;
use structopt::StructOpt;

const THUMBNAIL_LIMIT: u32 = 255;
const WHITE_THRESHOLD: u8 = 230;
const WARN_DISTANCE_THRESHOLD: u32 = 10;

fn is_pixel_white(pixel: &Rgba<u8>) -> bool {
    let data = pixel.0;
    data[0] > WHITE_THRESHOLD && data[1] > WHITE_THRESHOLD && data[2] > WHITE_THRESHOLD
}

/// Returns a (x, y, width, height) indicating the inner image.
fn detect_inner_image_bounds(image: &DynamicImage) -> (u32, u32, u32, u32) {
    let (width, height) = image.dimensions();
    let width_check_interval = width / 4;
    let height_check_interval = height / 4;
    let width_checks = [
        width_check_interval,
        width_check_interval * 2,
        width_check_interval * 3,
    ];
    let height_checks = [
        height_check_interval,
        height_check_interval * 2,
        height_check_interval * 3,
    ];

    let mut min_x = width / 2;
    let mut max_x = width / 2;
    for height_check in height_checks.iter() {
        for x_check in 0..width_checks[0] {
            if !is_pixel_white(&image.get_pixel(x_check, *height_check)) {
                min_x = std::cmp::min(min_x, x_check);
                break;
            }
        }

        for x_check in (width_checks[2]..width).rev() {
            if !is_pixel_white(&image.get_pixel(x_check, *height_check)) {
                max_x = std::cmp::max(max_x, x_check);
                break;
            }
        }
    }

    let mut min_y = height / 2;
    let mut max_y = height / 2;
    for width_check in width_checks.iter() {
        for y_check in 0..height_checks[0] {
            if !is_pixel_white(&image.get_pixel(*width_check, y_check)) {
                min_y = std::cmp::min(min_y, y_check);
                break;
            }
        }

        for y_check in (height_checks[2]..height).rev() {
            if !is_pixel_white(&image.get_pixel(*width_check, y_check)) {
                max_y = std::cmp::max(max_y, y_check);
                break;
            }
        }
    }

    (min_x, min_y, max_x - min_x, max_y - min_y)
}

fn remove_borders(image: &DynamicImage) -> DynamicImage {
    let (x, y, width, height) = detect_inner_image_bounds(&image);
    image.crop_imm(x, y, width, height)
}

#[derive(Debug)]
struct PathPhash {
    file_name: OsString,
    phash: ImageHash,
}

#[derive(Debug)]
struct Match {
    thumb: OsString,
    fullsize: OsString,
    distance: u32,
}

fn load_phash(path: PathBuf, phashes_cache_dir: &Path, cleanup: bool) -> Result<PathPhash> {
    let hasher = HasherConfig::new().to_hasher();

    let file_name = path.file_name().expect("No file name.");
    let mut thumb_phash_file = PathBuf::from(&phashes_cache_dir);
    thumb_phash_file.push(file_name);
    let phash = if thumb_phash_file.exists() {
        let encoded = read_to_string(&thumb_phash_file)?;
        ImageHash::from_base64(&encoded)?
    } else {
        info!("Hashing: {}", &file_name.to_string_lossy());
        let mut img = image::open(&path)?;
        if cleanup {
            img = remove_borders(&img);
        };
        let img = &img.thumbnail(THUMBNAIL_LIMIT, THUMBNAIL_LIMIT);
        let phash = hasher.hash_image(img);

        let mut file = File::create(thumb_phash_file)?;
        file.write_all(phash.to_base64().as_bytes())?;
        phash
    };

    Ok(PathPhash {
        file_name: file_name.to_owned(),
        phash,
    })
}

fn load_phashes(
    source_files_dir: &Path,
    phashes_cache_dir: &Path,
    cleanup: bool,
) -> Result<Vec<PathPhash>> {
    info!(
        "Loading directory: {} (cache: {})",
        &source_files_dir.to_string_lossy(),
        &phashes_cache_dir.to_string_lossy()
    );

    let mut source_paths = Vec::new();
    for entry in fs::read_dir(&source_files_dir)? {
        let entry = entry?;
        let path = entry.path();
        source_paths.push(path);
    }
    let phashes: Result<Vec<_>> = source_paths
        .into_par_iter()
        .map(|path| load_phash(path, phashes_cache_dir, cleanup))
        .collect();
    Ok(phashes?)
}

fn match_thumbs(
    fullsize_directory: &Path,
    thumbnail_directory: &Path,
    cache_directory: &Path,
    output_directory: &Path,
) -> Result<()> {
    fs::create_dir_all(fullsize_directory)?;
    fs::create_dir_all(thumbnail_directory)?;
    fs::create_dir_all(output_directory)?;

    let cache_fullsize_directory = cache_directory.join("fullsize");
    let cache_thumbnail_directory = cache_directory.join("thumbnail");
    fs::create_dir_all(&cache_fullsize_directory)?;
    fs::create_dir_all(&cache_thumbnail_directory)?;

    let loading_start = Instant::now();
    let fullsize_phashes = load_phashes(fullsize_directory, &cache_fullsize_directory, false)?;
    let thumbs_phashes = load_phashes(thumbnail_directory, &cache_thumbnail_directory, true)?;
    info!(
        "Loading phashes took: {}s",
        loading_start.elapsed().as_secs()
    );

    for thumb_phash in thumbs_phashes.iter() {
        let mut output: Option<Match> = None;
        for fullsize_phash in fullsize_phashes.iter() {
            let distance = thumb_phash.phash.dist(&fullsize_phash.phash);
            let new_output = Match {
                fullsize: fullsize_phash.file_name.clone(),
                thumb: thumb_phash.file_name.clone(),
                distance,
            };

            output = match output {
                None => Some(new_output),
                Some(old_output) => Some(if new_output.distance < old_output.distance {
                    new_output
                } else {
                    old_output
                }),
            }
        }

        if let Some(output) = output {
            info!(
                "Matched: {} to {}",
                output.thumb.to_string_lossy(),
                output.fullsize.to_string_lossy()
            );
            if output.distance > WARN_DISTANCE_THRESHOLD {
                info!(
                    "Distance from {} to {} was {}, needs manual review",
                    output.thumb.to_string_lossy(),
                    output.fullsize.to_string_lossy(),
                    output.distance
                );
            }
            let mut source_file = PathBuf::from(fullsize_directory);
            source_file.push(&output.fullsize);
            let mut target_file = PathBuf::from(output_directory);
            target_file.push(&output.fullsize);
            fs::copy(source_file, target_file)?;
        }
    }

    Ok(())
}

#[derive(Debug, StructOpt)]
#[structopt(name = "find", about = "Find matching images from a large set.")]
struct Opt {
    /// Fullsize image files (to search through for a match).
    #[structopt(long = "fullsize", parse(from_os_str))]
    fullsize_directory: PathBuf,

    /// Thumbnail image files (to find a match for).
    #[structopt(long = "thumbnail", parse(from_os_str))]
    thumbnail_directory: PathBuf,

    #[structopt(long = "cache", parse(from_os_str))]
    cache_directory: PathBuf,

    /// Output directory.
    #[structopt(long = "output", parse(from_os_str))]
    output_directory: PathBuf,

    /// Number of threads.
    #[structopt(default_value = "4")]
    num_threads: usize,
}

fn main() -> Result<()> {
    env_logger::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let opt = Opt::from_args();

    rayon::ThreadPoolBuilder::new()
        .num_threads(opt.num_threads)
        .build_global()?;

    match_thumbs(
        &opt.fullsize_directory,
        &opt.thumbnail_directory,
        &opt.cache_directory,
        &opt.output_directory,
    )?;

    Ok(())
}
