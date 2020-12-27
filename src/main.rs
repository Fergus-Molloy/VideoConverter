extern crate ffmpeg_next as ffmpeg;
use clap::arg_enum;
use ffmpeg::codec::{self, Context, Parameters};
use ffmpeg::format::context::Input;
use ffmpeg::media::Type;
use std::collections::HashMap;
use structopt::StructOpt;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    ffmpeg::init()?;

    let opt = Opt::from_args();

    println!("{:?}", opt);

    // Squelch libav* errors
    unsafe {
        ffmpeg::ffi::av_log_set_level(ffmpeg::ffi::AV_LOG_FATAL);
    }

    let file = ffmpeg::format::input(&"/home/jamie/Videos/Inception/Inception_t16.mkv")?;

    let parsed = parse_stream_metadata(&file);
    let mappings = get_mappings(&parsed);
    let codecs = get_codecs(&parsed, &mappings);
    print_codec_mapping(&parsed, &codecs);

    return Ok(());
}

#[derive(StructOpt, Debug)]
#[structopt(name = "VideoConverter")]
struct Opt {
    /// Keep all streams, regardless of language metadata. [Not Yet Implemented]
    #[structopt(short, long)]
    all_streams: bool,

    /// Specify a CRF value to be passed to libx264 [Not Yet Implemented]
    #[structopt(long, default_value = "20")]
    crf: u8,

    /// Specify a crop filter. These are of the format 'crop=height:width:x:y' [Not Yet Implemented]
    #[structopt(long)]
    crop: Option<String>,

    /// Force deinterlacing of video [Not Yet Implemented]
    #[structopt(short, long)]
    deinterlace: bool,

    /// Disable automatic deinterlacing of video [Not Yet Implemented]
    #[structopt(short = "-D", long)]
    no_deinterlace: bool,

    /// Force reencoding of video [Not Yet Implemented]
    #[structopt(long)]
    force_reencode: bool,

    /// Use GPU accelerated encoding (nvenc). This produces HEVC. Requires an Nvidia 10-series gpu
    /// or later [Not Yet Implemented]
    #[structopt(short, long)]
    gpu: bool,

    /// Disable hardware-accelerated decoding [Not Yet Implemented]
    #[structopt(long)]
    no_hwaccel: bool,

    /// Do not actually perform the conversion [Not Yet Implemented]
    #[structopt(short, long)]
    simulate: bool,

    /// Specify libx264 tune. Incompatible with --gpu [Not Yet Implemented]
    #[structopt(short, long, possible_values = &Libx264Tune::variants(), case_insensitive=true)]
    tune: Option<Libx264Tune>,

    #[structopt(short, long)]
    verbose: bool,

    /// Write output to a log file [Not Yet Implemented]
    #[structopt(long)]
    log: bool,

    /// The path to operate on
    #[structopt(default_value = ".")]
    path: std::path::PathBuf,
}

arg_enum! {
    #[derive(Debug)]
    enum Libx264Tune {
        Film,
        Animation,
        Grain,
        Stillimage,
        Psnr,
        Ssim,
        Fastdecode,
        Zerolatency,
    }
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
                eprintln!("Error getting field order for stream {}: {:?}", index, x);
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

fn get_mappings(parsed: &Vec<StreamType>) -> Vec<usize> {
    let mut video_mappings: Vec<usize> = Vec::new();
    let mut audio_mappings: Vec<usize> = Vec::new();
    let mut subtitle_mappings: Vec<usize> = Vec::new();

    for stream in parsed.iter() {
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
        eprintln!("Erorr: File has {} video streams", num_vids);
        process::exit(1);
    }

    if audio_mappings.len() == 0 {
        // if no english streams are detected, just use all streams
        for stream in parsed.iter() {
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

    return video_mappings
        .into_iter()
        .chain(audio_mappings.into_iter().chain(subtitle_mappings.into_iter()))
        .collect();
}

fn get_codecs(parsed: &Vec<StreamType>, mappings: &Vec<usize>) -> HashMap<usize, Option<codec::Id>> {
    let mut video_codecs: HashMap<usize, Option<codec::Id>> = HashMap::new();
    let mut audio_codecs: HashMap<usize, Option<codec::Id>> = HashMap::new();
    let mut subtitle_codecs: HashMap<usize, Option<codec::Id>> = HashMap::new();

    for index in mappings {
        let index = *index;
        let stream = &parsed[index];
        match stream {
            StreamType::Video(video) => match video.codec {
                codec::Id::HEVC => {
                    video_codecs.insert(index, None);
                }
                codec::Id::H264 => {
                    video_codecs.insert(index, None);
                }
                _ => {
                    video_codecs.insert(index, Some(codec::Id::H264));
                }
            },
            StreamType::Audio(audio) => match audio.codec {
                codec::Id::FLAC => {
                    audio_codecs.insert(index, None);
                }
                codec::Id::AAC => {
                    audio_codecs.insert(index, None);
                }

                codec::Id::TRUEHD => {
                    audio_codecs.insert(index, Some(codec::Id::FLAC));
                }
                codec::Id::DTS => match audio.profile {
                    Some(codec::Profile::DTS(codec::profile::DTS::HD_MA)) => {
                        audio_codecs.insert(index, Some(codec::Id::FLAC));
                    }
                    _ => {
                        audio_codecs.insert(index, Some(codec::Id::AAC));
                    }
                },
                _ => {
                    audio_codecs.insert(index, Some(codec::Id::AAC));
                }
            },
            StreamType::Subtitle(subtitle) => match subtitle.codec {
                codec::Id::HDMV_PGS_SUBTITLE => {
                    subtitle_codecs.insert(index, None);
                }
                codec::Id::DVD_SUBTITLE => {
                    subtitle_codecs.insert(index, None);
                }
                _ => {
                    subtitle_codecs.insert(index, Some(codec::Id::SSA));
                }
            },
        }
    }

    return video_codecs
        .into_iter()
        .chain(audio_codecs.into_iter().chain(subtitle_codecs.into_iter()))
        .collect();
}

fn print_codec_mapping(parsed: &Vec<StreamType>, codecs: &HashMap<usize, Option<codec::Id>>) {
    for (index, codec) in codecs.iter() {
        let oldcodec = match &parsed[*index] {
            StreamType::Video(video) => video.codec,
            StreamType::Audio(audio) => audio.codec,
            StreamType::Subtitle(subtitle) => subtitle.codec,
        };
        let newcodec = match codec {
            None => oldcodec,
            Some(x) => *x,
        };
        print!("stream {}: {:?} -> {:?}", index, oldcodec, newcodec);
        if codec.is_none() {
            println!(" (copy)");
        } else {
            println!("");
        }
    }
}
