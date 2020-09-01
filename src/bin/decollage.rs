use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use log::{debug, info};
use opencv::{
    core::{self, Mat, Point, Point2f, Rect_, Scalar, Size, Vector},
    imgcodecs, imgproc,
    types::VectorOfMat,
};
use structopt::StructOpt;

/// Minimum area of subimage area detected. Increase to remove noise, decrease
/// to ensure all subimages are extracted.
const MIN_SUBIMAGE_AREA: f64 = 5000.0;
const WHITE_THRESHOLD: f64 = 210.0;

fn process_collage_page(
    path: &Path,
    output_directory: &Path,
    debug_directory: Option<&Path>,
) -> Result<()> {
    info!("Processing collage page: {}", path.to_string_lossy());
    let path_stem = path.file_stem().expect("No file stem.").to_string_lossy();

    let mut img = imgcodecs::imread(
        path.to_str().expect("OpenCV could not open input path."),
        opencv::imgcodecs::IMREAD_COLOR,
    )?;

    let mut grey = Mat::default()?;
    imgproc::cvt_color(&img, &mut grey, imgproc::COLOR_BGR2GRAY, 0)?;
    let mut blur = Mat::default()?;
    imgproc::median_blur(&grey, &mut blur, 5)?;
    // sharpen_kernel = np.array([[-1,-1,-1], [-1,9,-1], [-1,-1,-1]])
    // sharpen = cv2.filter2D(blur, -1, sharpen_kernel)
    //
    let mut threshold = Mat::default()?;
    imgproc::threshold(
        &blur,
        &mut threshold,
        WHITE_THRESHOLD,
        255.0,
        imgproc::THRESH_BINARY_INV,
    )?;
    let kernel =
        imgproc::get_structuring_element(imgproc::MORPH_RECT, Size::new(3, 3), Point::new(-1, -1))?;
    let mut open = Mat::default()?;
    imgproc::morphology_ex(
        &threshold,
        &mut open,
        imgproc::MORPH_OPEN,
        &kernel,
        Point::new(-1, -1),
        2,
        core::BORDER_CONSTANT,
        // This default might be wrong
        Scalar::default(),
    )?;
    let mut close = Mat::default()?;
    imgproc::morphology_ex(
        &open,
        &mut close,
        imgproc::MORPH_CLOSE,
        &kernel,
        Point::new(-1, -1),
        2,
        core::BORDER_CONSTANT,
        // This default might be wrong
        Scalar::default(),
    )?;

    let mut contours = VectorOfMat::default();
    imgproc::find_contours(
        &close,
        &mut contours,
        imgproc::RETR_EXTERNAL,
        imgproc::CHAIN_APPROX_SIMPLE,
        Point::default(),
    )?;

    let mut patch_number = 0;
    for contour in contours.iter() {
        let area = imgproc::contour_area(&contour, false)?;
        if area > MIN_SUBIMAGE_AREA {
            let Rect_ {
                x,
                y,
                width,
                height,
            } = imgproc::bounding_rect(&contour)?;
            let cx: f32 = x as f32 + width as f32 / 2.0;
            let cy: f32 = y as f32 + height as f32 / 2.0;
            let mut patch = Mat::default()?;
            imgproc::get_rect_sub_pix(
                &img,
                Size::new(width, height),
                Point2f::new(cx, cy),
                &mut patch,
                -1,
            )?;
            let mut output_path = PathBuf::from(output_directory);
            output_path.push(&format!("{}-{:02}.png", path_stem, patch_number));
            info!("Writing subimage: {}", output_path.to_string_lossy());
            imgcodecs::imwrite(&output_path.to_string_lossy(), &patch, &Vector::default())?;
            //         cv2.rectangle(image, (x, y), (x + w, y + h), (36,255,12), 2)
            patch_number += 1;

            imgproc::rectangle(
                &mut img,
                Rect_::from_points(Point::new(x, y), Point::new(x + width, y + height)),
                // green
                Scalar::new(0.0, 255.0, 0.0, 255.0),
                10,
                imgproc::LINE_8,
                0,
            )?;
        } else {
            debug!("Discarding subimage with area: {}", area);
        }
    }

    if let Some(debug_directory) = debug_directory {
        imgcodecs::imwrite(
            &debug_directory
                .join(&format!("{}-patches.png", path_stem))
                .to_string_lossy(),
            &img,
            &Vector::default(),
        )?;

        imgcodecs::imwrite(
            &debug_directory
                .join(&format!("{}-processed.png", path_stem))
                .to_string_lossy(),
            &close,
            &Vector::default(),
        )?;
    }

    Ok(())
}

#[derive(Debug, StructOpt)]
#[structopt(
    name = "decollage",
    about = "Pull sub images out of input images with a white background."
)]
struct Opt {
    /// Input files
    #[structopt(parse(from_os_str))]
    input_directory: PathBuf,

    /// Output directory.
    #[structopt(parse(from_os_str))]
    output_directory: PathBuf,

    /// Output directory for debug files.
    #[structopt(long = "debug", parse(from_os_str))]
    debug_directory: Option<PathBuf>,
}

fn main() -> Result<()> {
    env_logger::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let opt = Opt::from_args();

    for entry in fs::read_dir(&opt.input_directory)? {
        let entry = entry?;
        let path = entry.path();
        process_collage_page(
            &path,
            &opt.output_directory,
            opt.debug_directory
                .as_ref()
                .map(|pathbuf| pathbuf.as_path()),
        )?;
    }

    Ok(())
}
