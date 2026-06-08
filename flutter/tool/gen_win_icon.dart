// Generates the Windows app icon (windows/runner/resources/app_icon.ico) as a
// multi-resolution .ico from assets/wisp_square_logo.png.
//
// The runner's previous app_icon.ico held only a single 48x48 image, so Windows
// had nothing to show at the larger sizes used by Explorer, the Start menu, and
// Settings > Installed apps — leaving the icon blank. A proper .ico bundles
// several sizes (16..256) so every shell context renders crisply.
//
// Run from the flutter/ directory:
//   dart run tool/gen_win_icon.dart
import 'dart:io';

import 'package:image/image.dart' as img;

const _source = 'assets/wisp_square_logo.png';
const _output = 'windows/runner/resources/app_icon.ico';

// Standard Windows icon sizes. 256 is the .ico per-image maximum.
const _sizes = [16, 24, 32, 48, 64, 128, 256];

void main() {
  final bytes = File(_source).readAsBytesSync();
  final src = img.decodeImage(bytes);
  if (src == null) {
    stderr.writeln('Could not decode $_source');
    exit(1);
  }
  stdout.writeln('source $_source ${src.width}x${src.height}');

  final frames = [
    for (final s in _sizes)
      img.copyResize(
        src,
        width: s,
        height: s,
        interpolation: img.Interpolation.cubic,
      ),
  ];

  final ico = img.IcoEncoder().encodeImages(frames);
  File(_output).writeAsBytesSync(ico);
  stdout.writeln('wrote $_output (${ico.length} bytes, ${_sizes.join("/")})');
}
