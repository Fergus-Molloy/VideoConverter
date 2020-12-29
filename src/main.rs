extern crate ffmpeg_next as ffmpeg;
extern crate regex;

#[macro_use]
extern crate lazy_static;

mod interface;
mod util;

//use clap::AppSettings::ColoredHelp;
use ffmpeg::codec::{self, Context, Parameters};
use ffmpeg::format::context::Input;
use ffmpeg::media::Type;
use itertools::sorted;
use log::{debug, error, info};
use regex::Regex;
use simple_error::SimpleError;
use std::collections::HashMap;
use std::path::Path;
use std::process;
use structopt::StructOpt;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    ffmpeg::init()?;
    pretty_env_logger::init();

    lazy_static! {
        static ref EXEMPT_EXTENSION_REGEX: Regex = Regex::new(r"r\d+").unwrap();
    }

    let opt = interface::Opt::from_args();

    debug!("{:?}", opt);

    // Squelch libav* errors
    unsafe {
        ffmpeg::ffi::av_log_set_level(ffmpeg::ffi::AV_LOG_FATAL);
    }

    let (tv_mode, tv_show_title, tv_show_season, tv_show_episode) = interface::get_tv_options()?;

    debug!(
        "tv_mode: {}, tv_show_title: {:?}, tv_show_season: {:?}, tv_show_episode: {:?}.",
        tv_mode, tv_show_title, tv_show_season, tv_show_episode
    );

    let entries = sorted(
        std::fs::read_dir(&opt.path)?
            .map(|entry| entry.unwrap().path())
            .filter(|path| !path.is_dir())
            .filter(|path| {
                let filename = path.file_name().and_then(|x| x.to_str()).unwrap();
                filename.chars().nth(0).unwrap() != '.'
            })
            .filter(|path| {
                let file_extension = match path.extension().and_then(|x| x.to_str()) {
                    Some(x) => x,
                    None => {
                        return false;
                    }
                };
                let exempt_file_extensions = ["gif", "jpg", "md", "nfo", "png", "py", "rar", "sfv", "srr", "txt"];
                return !(exempt_file_extensions.contains(&file_extension) || EXEMPT_EXTENSION_REGEX.is_match(file_extension));
            }),
    );

    // prepare directory
    {
        let dir_to_make = if tv_mode {
            opt.path.join(format!("Season {:02}", tv_show_season.unwrap()))
        } else {
            opt.path.join("newfiles")
        };
        let dir_as_str: &str = dir_to_make.as_os_str().to_str().expect("Path contained invalid unicode.");

        if dir_to_make.is_dir() {
            info!("Directory '{}' already exists.", dir_as_str);
        } else {
            if opt.simulate {
                info!("Simulate mode: not creating directory '{}'", dir_as_str);
            } else {
                std::fs::create_dir(&dir_to_make)?;
                info!("Created directory '{}'.", dir_as_str);
            }
        }
    }

    for path in entries {
        println!("Current file: '{}'", path.as_os_str().to_str().expect("Path contained invalid unicode."));

        let input_filename = path.file_name().expect("Input filename is None").to_string_lossy();
        let input_ext = path.extension().expect("Input ext is None").to_string_lossy();
        let output_filename = input_filename.replace(input_ext.as_ref(), "mkv");

        let file = ffmpeg::format::input(&path)?;

        let parsed = parse_stream_metadata(&file);
        let mappings = get_mappings(&parsed)?;
        let codecs = get_codecs(&parsed, &mappings);
        print_codec_mapping(&parsed, &mappings, &codecs);
    }

    return Ok(());
}

#[derive(Debug)]
enum StreamType {
    Video(Video),
    Audio(Audio),
    Subtitle(Subtitle),
}

#[derive(Debug)]
enum FieldOrder {
    Progressive,
    Unknown,
    Interlaced,
}

#[derive(Debug)]
struct Video {
    index: usize,
    codec: codec::Id,
    field_order: FieldOrder,
}

impl Video {
    pub fn new(index: usize, codec_context: Context, codec_par: Parameters) -> Video {
        let codec = codec_par.id();

        let decoder = codec_context.decoder().video();
        let field_order = match unsafe { decoder.map(|x| (*x.as_ptr()).field_order) } {
            Ok(ffmpeg::ffi::AVFieldOrder::AV_FIELD_PROGRESSIVE) => FieldOrder::Progressive,
            Ok(ffmpeg::ffi::AVFieldOrder::AV_FIELD_TT) => FieldOrder::Interlaced,
            Ok(ffmpeg::ffi::AVFieldOrder::AV_FIELD_TB) => FieldOrder::Interlaced,
            Ok(ffmpeg::ffi::AVFieldOrder::AV_FIELD_BT) => FieldOrder::Interlaced,
            Ok(ffmpeg::ffi::AVFieldOrder::AV_FIELD_BB) => FieldOrder::Interlaced,
            Ok(ffmpeg::ffi::AVFieldOrder::AV_FIELD_UNKNOWN) => FieldOrder::Unknown,
            Err(x) => {
                error!("Error getting field order for stream {}: {:?}", index, x);
                FieldOrder::Unknown
            }
        };

        Video { index, codec, field_order }
    }
}

#[derive(Debug)]
struct Audio {
    index: usize,
    codec: codec::Id,
    lang: Option<String>,
    profile: Option<ffmpeg::codec::Profile>,
}

impl Audio {
    pub fn new(index: usize, codec_context: Context, codec_par: Parameters, metadata: ffmpeg::util::dictionary::Ref<'_>) -> Audio {
        let codec = codec_par.id();
        let lang = metadata.get("language").map(|f| f.to_string());
        let decoder = codec_context.decoder().audio();
        let profile = match decoder.map(|x| x.profile()) {
            Ok(codec::Profile::Unknown) => None,
            Ok(x) => Some(x),
            Err(_) => None,
        };

        Audio { index, codec, lang, profile }
    }
}

#[derive(Debug)]
struct Subtitle {
    index: usize,
    codec: codec::Id,
    lang: Option<String>,
}

impl Subtitle {
    pub fn new(index: usize, codec_par: Parameters, metadata: ffmpeg::util::dictionary::Ref<'_>) -> Subtitle {
        let codec = codec_par.id();
        let lang = metadata.get("language").map(|f| f.to_string());

        Subtitle { index, codec, lang }
    }
}

fn parse_stream_metadata(file: &Input) -> Vec<StreamType> {
    let mut out: Vec<StreamType> = Vec::new();
    for stream in file.streams() {
        let index = stream.index();
        let codec_context = stream.codec();
        let codec_parameters = stream.parameters();
        let tags = stream.metadata();
        //let explode = codec.codec().unwrap();
        match codec_context.medium() {
            Type::Video => {
                out.push(StreamType::Video(Video::new(index, codec_context, codec_parameters)));
            }
            Type::Audio => {
                out.push(StreamType::Audio(Audio::new(index, codec_context, codec_parameters, tags)));
            }
            Type::Subtitle => {
                out.push(StreamType::Subtitle(Subtitle::new(index, codec_parameters, tags)));
            }
            _ => {}
        };
    }
    return out;
}

fn get_mappings(parsed: &[StreamType]) -> Result<Vec<usize>, SimpleError> {
    let mut video_mappings: Vec<usize> = Vec::new();
    let mut audio_mappings: Vec<usize> = Vec::new();
    let mut subtitle_mappings: Vec<usize> = Vec::new();

    for stream in parsed {
        match stream {
            StreamType::Video(video) => {
                video_mappings.push(video.index);
            }
            StreamType::Audio(audio) => {
                if audio.lang == Some("eng".to_string()) {
                    audio_mappings.push(audio.index);
                }
            }
            StreamType::Subtitle(subtitle) => {
                if subtitle.lang == Some("eng".to_string()) {
                    subtitle_mappings.push(subtitle.index);
                }
            }
        }
    }

    if video_mappings.len() != 1 {
        let num_vids = video_mappings.len();
        return Err(SimpleError::new(format!("File has {} video streams", num_vids)));
    }

    if audio_mappings.len() == 0 {
        // if no english streams are detected, just use all streams
        for stream in parsed {
            match stream {
                StreamType::Audio(audio) => {
                    audio_mappings.push(audio.index);
                }
                _ => {}
            }
        }
    }

    if subtitle_mappings.len() == 0 {
        // if no english streams are detected, just use all streams
        for stream in parsed.iter() {
            match stream {
                StreamType::Subtitle(subtitle) => {
                    subtitle_mappings.push(subtitle.index);
                }
                _ => {}
            }
        }
    }

    Ok(video_mappings
        .into_iter()
        .chain(audio_mappings.into_iter())
        .chain(subtitle_mappings.into_iter())
        .collect())
}

fn get_codecs(parsed: &[StreamType], mappings: &[usize]) -> HashMap<usize, Option<codec::Id>> {
    use codec::Id::{AAC, DTS, DVD_SUBTITLE, FLAC, H264, HDMV_PGS_SUBTITLE, HEVC, SSA, TRUEHD};
    mappings
        .iter()
        .map(|&index| match &parsed[index] {
            StreamType::Video(video) => match video.codec {
                HEVC | H264 => (index, None),
                _ => (index, Some(H264)),
            },
            StreamType::Audio(audio) => match audio.codec {
                FLAC | AAC => (index, None),

                TRUEHD => (index, Some(FLAC)),
                DTS => match audio.profile {
                    Some(codec::Profile::DTS(codec::profile::DTS::HD_MA)) => (index, Some(FLAC)),
                    _ => (index, Some(AAC)),
                },
                _ => (index, Some(AAC)),
            },
            StreamType::Subtitle(subtitle) => match subtitle.codec {
                HDMV_PGS_SUBTITLE | DVD_SUBTITLE => (index, None),
                _ => (index, Some(SSA)),
            },
        })
        .collect()
}

fn print_codec_mapping(parsed: &[StreamType], mappings: &[usize], codecs: &HashMap<usize, Option<codec::Id>>) {
    for index in mappings {
        let codec = codecs.get(&index).unwrap();
        let oldcodec = match &parsed[*index] {
            StreamType::Video(video) => &video.codec,
            StreamType::Audio(audio) => &audio.codec,
            StreamType::Subtitle(subtitle) => &subtitle.codec,
        };
        let newcodec = match codec {
            None => &oldcodec,
            Some(x) => x,
        };
        print!("stream {}: {:?} -> {:?}", index, oldcodec, newcodec);
        if codec.is_none() {
            println!(" (copy)");
        } else {
            println!("");
        }
    }
}
