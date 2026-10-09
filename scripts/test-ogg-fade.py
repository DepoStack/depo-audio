#!/usr/bin/env python3
"""Synthetic FFmpeg command regression; no customer audio or Rust runtime.

Run with installed ffmpeg/ffprobe: python3 scripts/test-ogg-fade.py
--legacy-duration demonstrates the pre-fix silent-tail assertion failure.
This exercises the Rust command's literal output options, not Tauri IPC.
"""
import argparse
import array
import json
import math
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
LEGACY = False


def run(args):
    return subprocess.run(args, capture_output=True, check=True, timeout=30).stdout


def duration(path):
    source = (ROOT / 'src-tauri/src/ffmpeg.rs').read_text()
    match = re.search(r'const OGG_DURATION_OUTPUT_ARGS: &\[&str\] = &\[(.*?)\];', source, re.S)
    if LEGACY:
        data = json.loads(run(['ffprobe', '-v', 'quiet', '-show_format', '-of', 'json', str(path)]))
        return float(data['format']['duration'])
    assert match, "Missing production Ogg measurement command"
    options = re.findall(r'"([^"\n]*)"', match.group(1))
    # Tiny generated fixtures only: communicate via run imposes a real timeout
    # even if decoding stalls. Production counts/discards bounded raw events;
    # this subprocess test does not emulate Tauri cancellation or memory use.
    count = len(run(['ffmpeg', '-nostdin', '-hide_banner', '-v', 'error',
                     '-protocol_whitelist', 'file', '-i', str(path)] + options))
    assert count > 0
    return count / 48000


def samples(path, fade=None, trim=False, channel=None, rate=48000, processing=()):
    filters = [] if channel is None else [f'pan=mono|c0=c{channel}']
    filters.extend(processing)
    if fade is not None:
        filters.append(f'afade=t=out:st={max(0, fade - .5):.3f}:d=0.5')
    if trim:
        filters.append('silenceremove=start_periods=1:start_duration=0.3:start_threshold=-50dB:start_mode=any')
    if fade is not None:
        filters.append('afade=t=in:d=0.5')
    args = ['ffmpeg', '-v', 'error', '-i', str(path), '-map', '0:a:0', '-vn', '-sn', '-dn']
    if filters:
        args += ['-af', ','.join(filters)]
    data = array.array('f')
    data.frombytes(run(args + ['-ar', str(rate), '-ac', '1', '-f', 'f32le', 'pipe:1']))
    return data


def rms(data):
    return math.sqrt(sum(x*x for x in data) / len(data))


class OggFade(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix='depo-ogg-regression-')
        cls.root = Path(cls.temp.name)
        cls.first, cls.second, cls.chain = [cls.root / n for n in ('first.ogg', 'second.ogg', 'chain.ogg')]
        for path, freq in [(cls.first, 440), (cls.second, 630)]:
            run(['ffmpeg', '-v', 'error', '-f', 'lavfi', '-i',
                 f'aevalsrc=0.1*sin(2*PI*{freq}*t)|0.1*sin(2*PI*{freq+100}*t)|0.1*sin(2*PI*{freq+200}*t)|0.1*sin(2*PI*{freq+300}*t):s=48000:d=2:c=quad',
                 '-c:a', 'libvorbis', '-y', str(path)])
        # FFmpeg independently generates distinct stream serials; verify that
        # precondition instead of mistaking duplicate streams for valid chaining.
        assert cls.first.read_bytes()[14:18] != cls.second.read_bytes()[14:18]
        cls.chain.write_bytes(cls.first.read_bytes() + cls.second.read_bytes())
        cls.silence = cls.root / 'leading-silence.ogg'
        run(['ffmpeg', '-v', 'error', '-f', 'lavfi', '-i',
             'aevalsrc=if(lt(t\\,0.6)\\,0\\,0.1*sin(2*PI*440*t)):s=48000:d=2',
             '-c:a', 'libvorbis', '-y', str(cls.silence)])

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def test_chain_keeps_later_audio_and_fades_only_at_decoded_end(self):
        plain = samples(self.chain)
        faded = samples(self.chain, duration(self.chain))
        self.assertEqual(len(plain), len(faded))
        self.assertGreater(len(plain), 190000)
        # Later speech before the final half-second must remain unchanged.
        self.assertEqual(plain[120000:160000], faded[120000:160000])
        self.assertLess(rms(faded[-1000:]), rms(plain[-1000:]) * .08)


    def test_single_stream_retains_existing_fade_samples(self):
        self.assertEqual(duration(self.first), 2.0)
        self.assertEqual(samples(self.first, duration(self.first)), samples(self.first, 2.0))

    def test_all_four_split_channels_keep_the_shared_timeline(self):
        measured = duration(self.chain)
        for channel in range(4):
            with self.subTest(channel=channel):
                plain = samples(self.chain, channel=channel)
                faded = samples(self.chain, measured, channel=channel)
                self.assertEqual(len(plain), len(faded))
                self.assertEqual(plain[120000:160000], faded[120000:160000])
                self.assertLess(rms(faded[-1000:]), rms(plain[-1000:]) * .08)

    def test_leading_trim_keeps_fade_at_original_source_end(self):
        measured = duration(self.silence)
        plain = samples(self.silence, trim=True)
        faded = samples(self.silence, measured, trim=True)
        self.assertEqual(len(plain), len(faded))
        self.assertLess(len(plain), 72000)
        self.assertLess(rms(faded[-1000:]), rms(plain[-1000:]) * .08)

    def test_normalization_and_44100_output_preserve_later_audio(self):
        processing = ['highpass=f=80', 'volume=1.5', 'loudnorm=I=-16:TP=-1.5:LRA=11']
        plain = samples(self.chain, rate=44100, processing=processing)
        faded = samples(self.chain, duration(self.chain), rate=44100, processing=processing)
        self.assertEqual(len(plain), len(faded))
        self.assertEqual(plain[110250:141120], faded[110250:141120])
        self.assertLess(rms(faded[-900:]), rms(plain[-900:]) * .08)

    def test_invalid_ogg_is_a_measurement_failure(self):
        invalid = self.root / 'invalid.ogg'
        invalid.write_bytes(b'OggS' + bytes(100))
        with self.assertRaises(subprocess.CalledProcessError):
            duration(invalid)

    def test_production_wiring_and_failure_policy(self):
        # Static assertions deliberately labeled as such: they do not exercise
        # native IPC, but prevent this command regression from silently testing
        # an unused constant while the real conversion still trusts metadata.
        source = (ROOT / 'src-tauri/src/ffmpeg.rs').read_text()
        conversion = (ROOT / 'src-tauri/src/conversion.rs').read_text()
        builder = source.split('pub(crate) async fn build_proc_filters_with_gain', 1)[1].split('/// Pure core', 1)[0]
        self.assertIn('opts.fade && feed_is_ogg(feed)?', builder)
        self.assertIn('Some(decoded_ogg_duration(app, feed, opts.ffmpeg_timeout as u64, cancelled).await?)', builder)
        self.assertIn('.set_raw_out(true)', source)
        self.assertIn('return counted_ogg_duration(bytes, status.code == Some(0))', source)
        self.assertIn('let proc = proc?;', conversion)
        self.assertLess(conversion.index('let proc = proc?;'), conversion.index('let mut output_cleanup'))

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--legacy-duration', action='store_true')
    args, remaining = parser.parse_known_args()
    LEGACY = args.legacy_duration
    unittest.main(argv=[__file__] + remaining)
