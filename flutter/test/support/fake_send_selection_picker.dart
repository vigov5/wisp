import 'package:app/features/send/application/model.dart';
import 'package:app/features/send/application/send_selection_picker.dart';

class FakeSendSelectionPicker implements SendSelectionPicker {
  FakeSendSelectionPicker({
    this.filesResult = const [],
    this.folderResult = const [],
    this.photosResult = const [],
  });

  final List<SendPickedFile> filesResult;
  final List<SendPickedFile> folderResult;
  final List<SendPickedFile> photosResult;
  int filesPickCount = 0;
  int folderPickCount = 0;
  int photosPickCount = 0;

  @override
  Future<List<SendPickedFile>> pickFiles() async {
    filesPickCount += 1;
    return filesResult;
  }

  @override
  Future<List<SendPickedFile>> pickFolder() async {
    folderPickCount += 1;
    return folderResult;
  }

  @override
  Future<List<SendPickedFile>> pickPhotos() async {
    photosPickCount += 1;
    return photosResult;
  }
}
