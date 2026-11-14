// The two Apple-only queries the CI scripts make, built by lib.sh's `ci_probe`:
//   apple-probe window <pid>   the window number of the process's largest on-screen window, for
//                              `screencapture -l`; exits 1 while it has none
//   apple-probe ocr <png>      the text Vision reads in an image, one line per observation, top to
//                              bottom: the text check on every capture (no pixel comparison)
// Listing windows by owner needs no Screen Recording permission (only their titles would).
import AppKit
import Vision

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(2)
}

let arguments = CommandLine.arguments
guard arguments.count == 3 else { fail("usage: apple-probe window <pid> | ocr <png>") }

switch arguments[1] {
case "window":
    guard let pid = Int(arguments[2]) else { fail("pid must be a number") }
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] ?? []
    let sized: [(id: Int, area: CGFloat)] = windows.compactMap { window in
        guard (window[kCGWindowOwnerPID as String] as? Int) == pid,
              (window[kCGWindowLayer as String] as? Int) == 0,
              let id = window[kCGWindowNumber as String] as? Int,
              let bounds = window[kCGWindowBounds as String] as? NSDictionary,
              let rect = CGRect(dictionaryRepresentation: bounds as CFDictionary)
        else { return nil }
        return (id, rect.width * rect.height)
    }
    // A starting app can show a tiny placeholder window first; wait for a real one.
    guard let best = sized.max(by: { $0.area < $1.area }), best.area >= 200 * 200 else { exit(1) }
    print(best.id)
case "ocr":
    guard let image = NSImage(contentsOfFile: arguments[2]),
          let cgImage = image.cgImage(forProposedRect: nil, context: nil, hints: nil)
    else { fail("cannot read \(arguments[2])") }
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false
    do {
        try VNImageRequestHandler(cgImage: cgImage).perform([request])
    } catch {
        fail("text recognition failed: \(error)")
    }
    // Vision's boxes are normalised with the origin at the bottom left.
    let lines = (request.results ?? []).sorted {
        let (a, b) = ($0.boundingBox, $1.boundingBox)
        return abs(a.midY - b.midY) > 0.01 ? a.midY > b.midY : a.minX < b.minX
    }
    for line in lines {
        if let text = line.topCandidates(1).first?.string { print(text) }
    }
default:
    fail("unknown command \(arguments[1])")
}
