import 'package:app/features/settings/presentation/widgets/settings_path_display.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('formats sandboxed download roots for display', () {
    expect(
      formatSettingsDownloadRootForDisplay(
        '/Users/samarh/Library/Containers/com.example.app/Data/Downloads/Wisp',
      ),
      '/Users/samarh/Downloads/Wisp',
    );
  });

  test('leaves normal paths unchanged', () {
    expect(
      formatSettingsDownloadRootForDisplay('/Users/samarh/Downloads/Wisp'),
      '/Users/samarh/Downloads/Wisp',
    );
  });
}
