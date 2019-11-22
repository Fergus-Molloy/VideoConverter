#!/bin/python
import copy
import json
import os
import subprocess
import sys


def log(i: str):
    if "-V" in sys.argv:
        with open("./videoconverter.log", "a") as f:
            f.write(i)
            f.write("\n")
        print(i)
    elif "-v" in sys.argv:
        print(i)


def encode(filename: str, outname: str, video_codec: str, crf: int, deinterlace: bool, others: list = None):
    cpu = not "nvenc" in video_codec
    log(filename)
    if others is None:
        others = []
    filters = []

    command = ["ffmpeg", "-hide_banner"]  # Hide the GPL blurb
    command += ["-hwaccel", "auto"] if (not "--no-hwaccel" in sys.argv) else []  # Enable hardware acceleration
    command += ["-threads", "0"]  # Max CPU threads
    command += ["-i", filename, "-max_muxing_queue_size", "16384"]  # Input file
    command += ["-c:v", video_codec, "-c:a", "copy", "-c:s", "copy"]  # Specify codecs
    command += ["-cutoff", "18000", "-vbr", "5"]  # audio information
    command += ["-crf", str(crf)] if (video_codec != "copy" and cpu) else []  # Set CRF
    command += ["-tune", sys.argv[sys.argv.index("--tune") + 1]] if ("--tune" in sys.argv) else []  # Specify libx264 tune

    command += ["-profile:v", "high", "-rc-lookahead", "250", "-preset", "slow"] if (video_codec == "libx264") else []  # Libx264 options
    command += ["-rc", "constqp", "-qp", str(crf), "-preset", "slow", "-profile:v", "main", "-b:v", "0", "-rc-lookahead", "32"] if not cpu else []
    filters += [sys.argv[sys.argv.index("--crop") + 1]] if ("--crop" in sys.argv) else []  # Crop filter
    filters += (["yadif"] if cpu else ["hwupload_cuda", "yadif_cuda"]) if deinterlace else []  # Deinterlacing filter

    command += ["-filter:v", ",".join(filters)] if (filters != []) else []  # apply filters

    command += others
    command += [outname]
    print("\n")
    print(*command, "\n")
    if "--sim" in sys.argv or "-s" in sys.argv:
        return
    subprocess.run(command)


def prepare_directory(directory):
    global season, TV
    outdir = f"Season {season:02}" if TV else "newfiles"
    os.chdir(directory)
    mkdir(outdir)
    return outdir


def clean_name(filename: str):
    return filename[:filename.rfind(".")] + ".mkv"


def mkdir(name="newfiles"):
    if not os.path.isdir(name):
        os.mkdir(name)


def main(directory: str):
    outdir = prepare_directory(directory)
    global episode
    global TV
    filelist: list = os.listdir(directory)
    log(filelist)
    filelist.sort(key=lambda s: s.casefold())
    log(filelist)
    exempt_strings = [".txt", ".rar", ".nfo", ".sfv", ".jpg", ".png", ".gif"]
    exempt_strings.extend([f".r{x:02}" for x in range(100)])
    for filename in filelist:
        parsed_info = {"video": {}, "audio": {}, "subtitle": {}}
        if os.path.isdir(filename):
            continue
        if not "." in filename:
            continue
        if any(ext in filename for ext in exempt_strings):
            continue
        if os.path.isdir("./" + filename):
            continue
        if TV:
            episode += 1
        file_info = json.loads(subprocess.check_output(["ffprobe", "-v", "quiet", "-print_format", "json", "-show_format", "-show_streams", filename]))
        log(file_info)
        streams: list = file_info["streams"]

        for stream in streams:
            if "video" in stream["codec_type"]:
                parsed_info["video"][stream["index"]] = stream
            if "audio" in stream["codec_type"]:
                parsed_info["audio"][stream["index"]] = stream
            if "subtitle" in stream["codec_type"]:
                parsed_info["subtitle"][stream["index"]] = stream

        for k, v in copy.deepcopy(parsed_info)["video"].items():
            if "mjpeg" in v["codec_name"] or "png" in v["codec_name"]:
                parsed_info["video"].pop(k)

        # video starts
        if len(parsed_info["video"]) > 1:
            raise KeyError("The file provided has more than one video stream")
        video_stream = list(parsed_info["video"].keys())[0]
        video_codec = "libx264"
        if "h264" in list(parsed_info["video"].values())[0]["codec_name"]:
            video_codec = "copy"
        elif "hevc" in list(parsed_info["video"].values())[0]["codec_name"]:
            video_codec = "copy"
        upscale: bool = False
        # if not parsed_info["video"][video_stream]["height"] >= 700:
        # if "--upscale" in sys.argv:
        # upscale = True
        video_mapping = [list(parsed_info["video"].keys())[0]]
        # video ends

        # audio starts
        audio_mapping = []
        try:
            if len(parsed_info["audio"]) <= 1:
                audio_mapping = list(parsed_info["audio"].keys())
            else:  # check for eng
                for k, i in parsed_info["audio"].items():
                    for v in i["tags"].values():
                        if "eng" in str(v):
                            audio_mapping.append(int(k))
                            break
        except KeyError:
            audio_mapping = list(parsed_info["audio"].keys())

        audio_mapping = list(set(audio_mapping))
        audio_mapping.sort()

        audio_codecs = {}
        for k, v in parsed_info["audio"].items():
            try:
                if "truehd" in v["codec_name"].lower():
                    audio_codecs[k] = "flac"
                    continue
                if ("dts" in v["profile"].lower()) and ("ma" in v["profile"].lower()):
                    audio_codecs[k] = "flac"
                    continue
            except KeyError:
                pass
            if "aac" in v["codec_name"] or "flac" in v["codec_name"]:
                audio_codecs[k] = "copy"
            else:
                audio_codecs[k] = "libfdk_aac"
        # audio ends

        # subtitle starts
        subtitle_mapping = []
        if len(parsed_info["subtitle"]) <= 1:
            subtitle_mapping = list(parsed_info["subtitle"].keys())
        else:  # check for eng. if there are no eng streams, and one or more streams have no metadata, add all
            for k, i in parsed_info["subtitle"].items():
                try:
                    for v in i["tags"].values():
                        if "eng" in str(v):
                            subtitle_mapping.append(int(k))
                            break
                except KeyError:
                    continue
            if len(subtitle_mapping) == 0:
                subtitle_mapping = list(parsed_info["subtitle"].keys())

        subtitle_mapping = list(set(subtitle_mapping))
        subtitle_mapping.sort()

        subtitle_codecs = {}
        for k, v in parsed_info["subtitle"].items():
            if ("pgs" in v["codec_name"]) or ("dvd" in v["codec_name"]):
                subtitle_codecs[k] = "copy"
            else:
                subtitle_codecs[k] = "ass"
        # subtitle ends

        codec_cmds = []
        for c, i in enumerate(audio_mapping):
            codec_cmds.extend([f"-c:a:{c}", audio_codecs[i]])
        for c, i in enumerate(subtitle_mapping):
            codec_cmds.extend([f"-c:s:{c}", subtitle_codecs[i]])

        map_cmds = []
        for i in video_mapping:
            map_cmds.extend(["-map", f"0:{i}"])
        for i in audio_mapping:
            map_cmds.extend(["-map", f"0:{i}"])
        for i in subtitle_mapping:
            map_cmds.extend(["-map", f"0:{i}"])

        if TV:
            global title, season
            outname = f"{title} - s{season:02}e{episode:02}.mkv"
        else:
            outname = clean_name(filename)

        log(f"{filename} -> {outname}")
        global endStr
        endStr += f"{filename} -> {outname}\n"

        additional_cmds = codec_cmds + map_cmds
        crf = int(sys.argv[sys.argv.index("--crf") + 1]) if "--crf" in sys.argv else 20
        try:
            deinterlace = "progressive" not in parsed_info["video"][video_stream]["field_order"]
        except KeyError:
            deinterlace = False
        if "--force-reencode" in sys.argv:
            video_codec = "libx264"

        if "--gpu" in sys.argv:
            if video_codec == "libx264" or video_codec == "libx265":
                video_codec = "hevc_nvenc"

        encode(filename, f"{outdir}/{outname}", crf=crf, video_codec=video_codec, others=additional_cmds, deinterlace=deinterlace)


if __name__ == "__main__":
    if "-h" in sys.argv or "--help" in sys.argv:
        print("--help, -h", "Display this message", sep="\t\t")
        print("--no-hwaccel", "Disable hardware accelerated decoding", sep="\t\t")
        print("--tune <tune>", "Use libx264 tune <tune>. Does not work with --gpu", sep="\t\t")
        print("--crop <cropfilter>", "Use a crop filter", sep="\t")
        print("--simulate, -s", "Do everything apart from run the ffmpeg command", sep="\t\t")
        print("--crf", "Specify a CRF value", sep="\t\t\t")
        print("--force-reencode", "Rencode even if it is not needed", sep="\t")
        print("--gpu", "Use GPU accelerated encoding (produces hevc)", sep="\t\t\t")
        exit()
    else:
        TV = "n" not in input("TV show mode? (Y/n) ").lower()
        if TV:
            title = input("Please enter the title of the TV Show: ")
            season = int(input("Which season is this? "))
            episode = input("What is the first episode in this disc? (defaults to 1) ")
            episode = int(episode) - 1 if episode != "" else 0
        endStr = "\n"
        main(".")
        print(endStr)
