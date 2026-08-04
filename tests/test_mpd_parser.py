import pytest

from backend.app.downloader.mpd import (
    NativeDashUnsupported,
    expand_template,
    parse_iso_duration,
    parse_mpd,
)
from backend.app.downloader.parser import UnsupportedPlaylistError


def test_mpd_inherits_raw_playlist_token_to_relative_base_and_segments():
    mpd = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT4S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <Representation id="v" bandwidth="1" width="2" height="2">
        <BaseURL>video/</BaseURL>
        <SegmentTemplate media="$Number$.m4s" initialization="init.mp4"
          duration="4" timescale="1"/>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"""

    parsed = parse_mpd(
        "https://cdn.test/root/main.mpd?token=a%2Fb%2Bc&_HLS_msn=9",
        mpd,
    )

    assert parsed["video"]["init_url"] == (
        "https://cdn.test/root/video/init.mp4?token=a%2Fb%2Bc"
    )
    assert parsed["video"]["segments"][0]["url"] == (
        "https://cdn.test/root/video/1.m4s?token=a%2Fb%2Bc"
    )
MPD_NS = 'xmlns="urn:mpeg:dash:schema:mpd:2011"'


def test_parse_iso_duration_variants():
    assert parse_iso_duration("PT1H2M3.5S") == pytest.approx(3723.5)
    assert parse_iso_duration("PT30S") == pytest.approx(30.0)
    assert parse_iso_duration("P1DT1S") == pytest.approx(86401.0)
    assert parse_iso_duration("") == 0.0
    assert parse_iso_duration("garbage") == 0.0


def test_expand_template_formats_and_escapes():
    assert expand_template(
        "seg-$RepresentationID$-$Number%05d$.m4s", representation_id="v1", number=7
    ) == "seg-v1-00007.m4s"
    assert expand_template("t-$Time$.m4s", time=900000) == "t-900000.m4s"
    assert expand_template("cost$$$Bandwidth$", bandwidth=1200) == "cost$1200"


def test_segment_template_with_number_and_duration():
    mpd = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT10S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Number$.m4s" initialization="v/init.mp4"
        duration="4" timescale="1" startNumber="1"/>
      <Representation id="v720" bandwidth="2000000" width="1280" height="720"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4" lang="zh">
      <SegmentTemplate media="a/$RepresentationID$-$Number$.m4s" initialization="a/init.mp4"
        duration="4" timescale="1"/>
      <Representation id="a1" bandwidth="128000"/>
    </AdaptationSet>
  </Period>
</MPD>"""
    parsed = parse_mpd("https://cdn.test/stream/manifest.mpd", mpd)
    video = parsed["video"]
    audio = parsed["audio"]
    assert parsed["duration"] == pytest.approx(10.0)
    assert video["init_url"] == "https://cdn.test/stream/v/init.mp4"
    assert [seg["url"] for seg in video["segments"]] == [
        "https://cdn.test/stream/v/1.m4s",
        "https://cdn.test/stream/v/2.m4s",
        "https://cdn.test/stream/v/3.m4s",
    ]
    # The tail segment only covers the remaining 2 seconds.
    assert video["segments"][-1]["duration"] == pytest.approx(2.0)
    assert audio["lang"] == "zh"
    assert audio["segments"][0]["url"] == "https://cdn.test/stream/a/a1-1.m4s"


def test_segment_timeline_with_time_template_and_repeat():
    mpd = f"""<MPD {MPD_NS} type="static">
  <Period duration="PT12S">
    <AdaptationSet mimeType="video/mp4">
      <Representation id="v" bandwidth="1000" width="640" height="360">
        <SegmentTemplate media="s-$Time$.m4s" timescale="90000">
          <SegmentTimeline>
            <S t="0" d="360000" r="1"/>
            <S d="360000"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"""
    parsed = parse_mpd("https://cdn.test/live/main.mpd", mpd)
    urls = [seg["url"] for seg in parsed["video"]["segments"]]
    assert urls == [
        "https://cdn.test/live/s-0.m4s",
        "https://cdn.test/live/s-360000.m4s",
        "https://cdn.test/live/s-720000.m4s",
    ]
    assert parsed["video"]["segments"][0]["duration"] == pytest.approx(4.0)


def test_open_ended_timeline_repeat_fills_period_duration():
    mpd = f"""<MPD {MPD_NS} type="static">
  <Period duration="PT8S">
    <AdaptationSet mimeType="video/mp4">
      <Representation id="v" bandwidth="1000" width="640" height="360">
        <SegmentTemplate media="s-$Time$.m4s" timescale="1">
          <SegmentTimeline>
            <S t="0" d="2" r="-1"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"""
    parsed = parse_mpd("https://cdn.test/vod/main.mpd", mpd)
    urls = [seg["url"] for seg in parsed["video"]["segments"]]
    # r="-1" repeats until the 8-second period is filled: 4 two-second parts.
    assert urls == [
        "https://cdn.test/vod/s-0.m4s",
        "https://cdn.test/vod/s-2.m4s",
        "https://cdn.test/vod/s-4.m4s",
        "https://cdn.test/vod/s-6.m4s",
    ]


def test_thumbnail_image_tracks_never_win_video_selection():
    mpd = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT100S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="v/$Number$.m4s" duration="4" timescale="1"/>
      <Representation id="v720" bandwidth="2500000" width="1280" height="720" codecs="avc1.64001f"/>
    </AdaptationSet>
    <AdaptationSet mimeType="image/jpeg">
      <SegmentTemplate media="thumbs/tile_$Number$.jpg" duration="25" timescale="1"/>
      <Representation id="thumbs" bandwidth="12000" width="1600" height="900"/>
    </AdaptationSet>
    <AdaptationSet contentType="image">
      <SegmentTemplate media="sprite/$Number$.jpg" duration="25" timescale="1"/>
      <Representation id="sprites" bandwidth="9000" width="3200" height="1800"/>
    </AdaptationSet>
  </Period>
</MPD>"""
    parsed = parse_mpd("https://cdn.test/vod/main.mpd", mpd)
    # The tile grid advertises 1600x900 / 3200x1800 but must never outrank
    # the real 720p video track.
    assert parsed["video"]["id"] == "v720"
    assert parsed["audio"] is None


def test_open_repeat_honors_presentation_time_offset():
    mpd = f"""<MPD {MPD_NS} type="static">
  <Period duration="PT1H">
    <AdaptationSet mimeType="video/mp4">
      <Representation id="v" bandwidth="1000" width="640" height="360">
        <SegmentTemplate media="v/$Time$.m4s" initialization="v/init.mp4"
          timescale="90000" presentationTimeOffset="900000000000">
          <SegmentTimeline>
            <S t="900000000000" d="540000" r="-1"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"""
    parsed = parse_mpd("https://cdn.test/vod/main.mpd", mpd)
    segments = parsed["video"]["segments"]
    # A live-to-VOD timeline keeps its epoch-scale origin: one hour at six
    # seconds per segment is 600 segments, not one.
    assert len(segments) == 600
    assert segments[0]["url"].endswith("v/900000000000.m4s")
    assert segments[-1]["url"].endswith(f"v/{900000000000 + 599 * 540000}.m4s")


def test_open_repeat_stops_at_next_timeline_row():
    mpd = f"""<MPD {MPD_NS} type="static">
  <Period duration="PT12S">
    <AdaptationSet mimeType="video/mp4">
      <Representation id="v" bandwidth="1000" width="640" height="360">
        <SegmentTemplate media="s-$Time$.m4s" timescale="1">
          <SegmentTimeline>
            <S t="0" d="2" r="-1"/>
            <S t="8" d="4"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"""
    parsed = parse_mpd("https://cdn.test/vod/main.mpd", mpd)
    urls = [seg["url"].rsplit("/", 1)[-1] for seg in parsed["video"]["segments"]]
    # r="-1" runs until the next row's t=8, then the explicit row follows —
    # no phantom s-10 and no duplicated s-8.
    assert urls == ["s-0.m4s", "s-2.m4s", "s-4.m4s", "s-6.m4s", "s-8.m4s"]


def test_adaptation_level_segment_list_is_inherited():
    mpd = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT8S">
  <BaseURL>https://cdn.example/root/</BaseURL>
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentList duration="4" timescale="1">
        <Initialization sourceURL="init.mp4"/>
        <SegmentURL media="p1.m4s"/>
        <SegmentURL media="p2.m4s"/>
      </SegmentList>
      <Representation id="v" bandwidth="900000" width="640" height="360"/>
    </AdaptationSet>
  </Period>
</MPD>"""
    parsed = parse_mpd("https://origin.test/main.mpd", mpd)
    track = parsed["video"]
    assert track["init_url"] == "https://cdn.example/root/init.mp4"
    assert [seg["url"] for seg in track["segments"]] == [
        "https://cdn.example/root/p1.m4s",
        "https://cdn.example/root/p2.m4s",
    ]
    assert track["single_file"] is False


def test_ancestor_base_url_alone_is_not_a_single_file():
    mpd = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT8S">
  <BaseURL>https://cdn.example/root/</BaseURL>
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <Representation id="v" bandwidth="900000" width="640" height="360"/>
    </AdaptationSet>
  </Period>
</MPD>"""
    with pytest.raises(NativeDashUnsupported):
        parse_mpd("https://origin.test/main.mpd", mpd)


def test_segment_list_and_nested_base_urls():
    mpd = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT8S">
  <BaseURL>https://mirror.test/root/</BaseURL>
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <BaseURL>videos/</BaseURL>
      <Representation id="v" bandwidth="900" width="640" height="480">
        <SegmentList duration="4" timescale="1">
          <Initialization sourceURL="init.mp4"/>
          <SegmentURL media="part1.m4s"/>
          <SegmentURL media="part2.m4s"/>
        </SegmentList>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"""
    parsed = parse_mpd("https://cdn.test/a/manifest.mpd", mpd)
    video = parsed["video"]
    assert video["init_url"] == "https://mirror.test/root/videos/init.mp4"
    assert [seg["url"] for seg in video["segments"]] == [
        "https://mirror.test/root/videos/part1.m4s",
        "https://mirror.test/root/videos/part2.m4s",
    ]


def test_single_file_representation_is_supported():
    mpd = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT60S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <Representation id="v" bandwidth="5000" width="1920" height="1080">
        <BaseURL>movie-video.mp4</BaseURL>
        <SegmentBase indexRange="0-999"/>
      </Representation>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <Representation id="a" bandwidth="128">
        <BaseURL>movie-audio.mp4</BaseURL>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"""
    parsed = parse_mpd("https://cdn.test/movie/manifest.mpd", mpd)
    assert parsed["video"]["single_file"] is True
    assert parsed["video"]["segments"] == [
        {"url": "https://cdn.test/movie/movie-video.mp4", "duration": pytest.approx(60.0)}
    ]
    assert parsed["audio"]["segments"][0]["url"] == "https://cdn.test/movie/movie-audio.mp4"


def test_highest_resolution_video_and_best_audio_selected():
    mpd = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT4S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <SegmentTemplate media="$RepresentationID$/$Number$.m4s" duration="4" timescale="1"/>
      <Representation id="v360" bandwidth="800000" width="640" height="360"/>
      <Representation id="v1080" bandwidth="4000000" width="1920" height="1080"/>
      <Representation id="v720" bandwidth="2500000" width="1280" height="720"/>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <SegmentTemplate media="$RepresentationID$/$Number$.m4s" duration="4" timescale="1"/>
      <Representation id="a-low" bandwidth="64000"/>
      <Representation id="a-high" bandwidth="192000"/>
    </AdaptationSet>
  </Period>
</MPD>"""
    parsed = parse_mpd("https://cdn.test/x/m.mpd", mpd)
    assert parsed["video"]["id"] == "v1080"
    assert parsed["audio"]["id"] == "a-high"


def test_drm_content_protection_is_rejected_hard():
    mpd = f"""<MPD {MPD_NS} type="static" mediaPresentationDuration="PT4S">
  <Period>
    <AdaptationSet mimeType="video/mp4">
      <ContentProtection schemeIdUri="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"/>
      <SegmentTemplate media="$Number$.m4s" duration="4" timescale="1"/>
      <Representation id="v" bandwidth="1" width="2" height="2"/>
    </AdaptationSet>
  </Period>
</MPD>"""
    with pytest.raises(UnsupportedPlaylistError):
        parse_mpd("https://cdn.test/m.mpd", mpd)


def test_dynamic_and_incompatible_multi_period_fall_back():
    dynamic = f'<MPD {MPD_NS} type="dynamic"><Period/></MPD>'
    with pytest.raises(NativeDashUnsupported):
        parse_mpd("https://cdn.test/m.mpd", dynamic)
    multi = f'''<MPD {MPD_NS} type="static">
  <Period duration="PT2S"><AdaptationSet mimeType="video/mp4">
    <SegmentTemplate initialization="init-1.mp4" media="one-$Number$.m4s" duration="2"/>
    <Representation id="v" bandwidth="1" width="2" height="2"/>
  </AdaptationSet></Period>
  <Period duration="PT2S"><AdaptationSet mimeType="video/mp4">
    <SegmentTemplate initialization="init-2.mp4" media="two-$Number$.m4s" duration="2"/>
    <Representation id="v" bandwidth="1" width="2" height="2"/>
  </AdaptationSet></Period>
</MPD>'''
    with pytest.raises(NativeDashUnsupported):
        parse_mpd("https://cdn.test/m.mpd", multi)
    with pytest.raises(NativeDashUnsupported):
        parse_mpd("https://cdn.test/m.mpd", "#EXTM3U not xml")


def test_compatible_static_multi_period_is_flattened_for_native_segments():
    manifest = f'''<MPD {MPD_NS} type="static">
  <BaseURL>https://cdn.test/media/</BaseURL>
  <Period id="p1" duration="PT2S"><AdaptationSet mimeType="video/mp4">
    <SegmentTemplate initialization="init.mp4" media="one-$Number$.m4s" duration="2"/>
    <Representation id="v" bandwidth="1000" width="640" height="360"/>
  </AdaptationSet></Period>
  <Period id="p2" duration="PT2S"><AdaptationSet mimeType="video/mp4">
    <SegmentTemplate initialization="init.mp4" media="two-$Number$.m4s" duration="2"/>
    <Representation id="v" bandwidth="1000" width="640" height="360"/>
  </AdaptationSet></Period>
</MPD>'''
    parsed = parse_mpd("https://origin.test/main.mpd", manifest)
    assert parsed["period_count"] == 2
    assert parsed["duration"] == pytest.approx(4.0)
    assert parsed["video"]["init_url"] == "https://cdn.test/media/init.mp4"
    assert [item["url"] for item in parsed["video"]["segments"]] == [
        "https://cdn.test/media/one-1.m4s",
        "https://cdn.test/media/two-1.m4s",
    ]
    assert [item["period_index"] for item in parsed["video"]["segments"]] == [0, 1]
    assert [item["start"] for item in parsed["video"]["segments"]] == [0.0, 2.0]


def test_multi_period_without_period_durations_uses_segment_timeline_offsets():
    manifest = f'''<MPD {MPD_NS} type="static">
  <Period><AdaptationSet mimeType="video/mp4">
    <SegmentTemplate initialization="init.mp4" media="one-$Number$.m4s" timescale="1"><SegmentTimeline><S t="0" d="2"/></SegmentTimeline></SegmentTemplate>
    <Representation id="v" bandwidth="1000" width="640" height="360"/>
  </AdaptationSet></Period>
  <Period><AdaptationSet mimeType="video/mp4">
    <SegmentTemplate initialization="init.mp4" media="two-$Number$.m4s" timescale="1"><SegmentTimeline><S t="0" d="2"/></SegmentTimeline></SegmentTemplate>
    <Representation id="v" bandwidth="1000" width="640" height="360"/>
  </AdaptationSet></Period>
</MPD>'''
    parsed = parse_mpd("https://origin.test/main.mpd", manifest)
    assert parsed["duration"] == pytest.approx(4.0)
    assert [item["start"] for item in parsed["video"]["segments"]] == [0.0, 2.0]


def test_multi_period_subtitles_merge_period_local_ids_and_use_timeline_offsets():
    manifest = f'''<MPD {MPD_NS} type="static">
  <Period><AdaptationSet mimeType="video/mp4">
    <SegmentTemplate initialization="init.mp4" media="one-$Number$.m4s" timescale="1"><SegmentTimeline><S t="0" d="2"/></SegmentTimeline></SegmentTemplate>
    <Representation id="v" bandwidth="1000" width="640" height="360"/>
  </AdaptationSet><AdaptationSet contentType="text" mimeType="text/vtt" lang="zh" label="中文">
    <Representation id="sub-p1"><SegmentList duration="2"><SegmentURL media="one.vtt"/></SegmentList></Representation>
  </AdaptationSet></Period>
  <Period><AdaptationSet mimeType="video/mp4">
    <SegmentTemplate initialization="init.mp4" media="two-$Number$.m4s" timescale="1"><SegmentTimeline><S t="0" d="2"/></SegmentTimeline></SegmentTemplate>
    <Representation id="v" bandwidth="1000" width="640" height="360"/>
  </AdaptationSet><AdaptationSet contentType="text" mimeType="text/vtt" lang="zh" label="中文">
    <Representation id="sub-p2"><SegmentList duration="2"><SegmentURL media="two.vtt"/></SegmentList></Representation>
  </AdaptationSet></Period>
</MPD>'''
    parsed = parse_mpd("https://origin.test/main.mpd", manifest)
    assert len(parsed["subtitle_tracks"]) == 1
    assert [item["start"] for item in parsed["subtitle_tracks"][0]["segments"]] == [0.0, 2.0]


def test_parse_exposes_dash_webvtt_subtitle_tracks():
    manifest = f"""<MPD {MPD_NS} mediaPresentationDuration="PT4S">
  <Period>
    <AdaptationSet mimeType="video/mp4"><Representation id="v" bandwidth="1000">
      <BaseURL>video.mp4</BaseURL>
    </Representation></AdaptationSet>
    <AdaptationSet contentType="text" mimeType="text/vtt" lang="zh" label="中文">
      <Representation id="sub-zh" bandwidth="100">
        <SegmentList duration="2" timescale="1">
          <SegmentURL media="sub-0.vtt"/><SegmentURL media="sub-1.vtt"/>
        </SegmentList>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"""

    parsed = parse_mpd("https://example.test/manifest.mpd", manifest)

    assert len(parsed["subtitle_tracks"]) == 1
    track = parsed["subtitle_tracks"][0]
    assert track["lang"] == "zh"
    assert track["name"] == "中文"
    assert [item["url"] for item in track["segments"]] == [
        "https://example.test/sub-0.vtt",
        "https://example.test/sub-1.vtt",
    ]
