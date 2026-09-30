#!/usr/bin/env python3
"""Validate the native writer's synthetic fixtures, including overlay placement.

The fixture is gray with white annotation text. This is a deterministic pixel
regression check, not OCR and not a claim that live desktop capture was tested.
"""
import array
import json
import math
from pathlib import Path
import subprocess
import sys


def main():
    directory = Path(sys.argv[1] if len(sys.argv) > 1 else 'artifacts')
    report = []
    for name, audio in [('synthetic-video-only.mp4', False), ('synthetic-with-audio.mp4', True)]:
        path = directory / name
        data = json.loads(subprocess.check_output(['ffprobe', '-v', 'error', '-show_streams', '-show_format', '-of', 'json', str(path)]))
        video = next(s for s in data['streams'] if s['codec_type'] == 'video')
        assert video['codec_name'] == 'h264', data
        width, height = video['width'], video['height']
        assert (width, height) == (640, 360), data
        assert abs(float(data['format']['duration']) - 1) < .1, data
        sounds = [s for s in data['streams'] if s['codec_type'] == 'audio']
        assert bool(sounds) == audio, data
        if audio:
            assert sounds[0]['codec_name'] == 'aac', data
            assert abs(float(sounds[0]['duration']) - 1) < .1, data

        pixels = subprocess.check_output(['ffmpeg', '-nostdin', '-v', 'error', '-i', str(path), '-frames:v', '1', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'])
        assert len(pixels) == width * height * 3

        def bright_pixels(start, end):
            count = 0
            for offset in range(start * width * 3, end * width * 3, 3):
                if min(pixels[offset:offset + 3]) > 200:
                    count += 1
            return count

        top = bright_pixels(0, 70)
        bottom = bright_pixels(height - 70, height)
        assert top > 100, f'{name}: Run/Step text is missing from the top region ({top} bright pixels)'
        assert bottom < 10, f'{name}: overlay incorrectly appears at the bottom ({bottom} bright pixels)'
        center = (height // 2 * width + width // 2) * 3
        assert all(65 <= value <= 95 for value in pixels[center:center + 3]), 'source background changed unexpectedly'
        subprocess.run(['ffmpeg', '-nostdin', '-y', '-v', 'error', '-i', str(path), '-frames:v', '1', str(directory / (path.stem + '.png'))], check=True)
        result = {'file': name, 'top_text_pixels': top, 'bottom_text_pixels': bottom}
        if audio:
            pcm = subprocess.check_output(['ffmpeg', '-nostdin', '-v', 'error', '-i', str(path), '-vn', '-ac', '1', '-f', 's16le', '-'])
            samples = array.array('h')
            samples.frombytes(pcm)
            if sys.byteorder != 'little':
                samples.byteswap()
            assert samples
            rms = math.sqrt(sum(value * value for value in samples) / len(samples))
            assert rms > 1000, f'encoded AAC track is unexpectedly silent (RMS {rms})'
            result['decoded_audio_rms'] = round(rms, 2)
        report.append(result)
    (directory / 'native-pixel-audio-report.json').write_text(json.dumps(report, indent=2))
    print('Native H.264/AAC duration, non-silent audio and top-left overlay pixel checks passed.')


if __name__ == '__main__':
    main()
