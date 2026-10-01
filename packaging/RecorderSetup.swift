import AppKit
import Foundation

enum SetupError: LocalizedError {
    case message(String)
    var errorDescription: String? {
        if case .message(let text) = self { return text }
        return nil
    }
}

final class CommandSetup {
    let home: URL
    let bundle: URL
    let manager = FileManager.default
    init(home: URL, bundle: URL) { self.home = home; self.bundle = bundle }

    func install() throws {
        let tools = bundle.appendingPathComponent("Contents/Resources/bin")
        for name in ["rec", "rec-capture", "ffmpeg", "ffprobe"] {
            guard manager.isExecutableFile(atPath: tools.appendingPathComponent(name).path) else {
                throw SetupError.message("설치 파일이 손상되었습니다. 앱을 다시 다운로드해 주세요.")
            }
        }
        let bin = home.appendingPathComponent(".local/bin", isDirectory: true)
        try manager.createDirectory(at: bin, withIntermediateDirectories: true)
        let suffix = ".rec-backup-\(UUID().uuidString)"
        for name in ["rec", "rec-capture"] {
            let link = bin.appendingPathComponent(name)
            let target = tools.appendingPathComponent(name).path
            if (try? manager.destinationOfSymbolicLink(atPath: link.path)) == target { continue }
            if (try? manager.attributesOfItem(atPath: link.path)) != nil {
                try manager.moveItem(at: link, to: bin.appendingPathComponent(name + suffix))
            }
            try manager.createSymbolicLink(atPath: link.path, withDestinationPath: target)
        }
        // New login shells use the CLI without requiring manual PATH instructions.
        let marker = "# Agent Work Recorder commands"
        for name in [".zprofile", ".bash_profile"] {
            let profile = home.appendingPathComponent(name)
            let type = try? manager.attributesOfItem(atPath: profile.path)[.type] as? FileAttributeType
            if type == .typeSymbolicLink {
                throw SetupError.message("\(name)이 링크입니다. 명령은 연결됐지만 셸 설정은 변경하지 않았습니다. 에이전트에 앱 안의 rec 경로를 전달해 주세요.")
            }
            let original = manager.fileExists(atPath: profile.path) ? try String(contentsOf: profile, encoding: .utf8) : ""
            if original.components(separatedBy: "\n").contains(marker) { continue }
            if manager.fileExists(atPath: profile.path) {
                try manager.copyItem(at: profile, to: home.appendingPathComponent(name + suffix))
            }
            let updated = original + (original.hasSuffix("\n") || original.isEmpty ? "" : "\n")
                + "\n\(marker)\nexport PATH=\"$HOME/.local/bin:$PATH\"\n"
            try updated.write(to: profile, atomically: true, encoding: .utf8)
        }
    }
}

final class SetupDelegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    var status: NSTextField!
    var installButton: NSButton!
    func applicationDidFinishLaunching(_ notification: Notification) {
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 600, height: 410),
            styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
        window.title = "Agent Work Recorder"
        window.center()
        let view = window.contentView!
        func label(_ text: String, frame: NSRect, size: CGFloat, weight: NSFont.Weight = .regular) -> NSTextField {
            let item = NSTextField(wrappingLabelWithString: text)
            item.frame = frame; item.font = .systemFont(ofSize: size, weight: weight)
            view.addSubview(item); return item
        }
        _ = label("작업 기록을 시작할 준비", frame: NSRect(x: 32, y: 330, width: 536, height: 40), size: 27, weight: .bold)
        _ = label("에이전트의 작업 과정을 영상으로 남깁니다.\n필요한 녹화·미디어 도구가 모두 포함되어 있습니다.", frame: NSRect(x: 32, y: 259, width: 536, height: 64), size: 17)
        _ = label("1. 앱을 응용 프로그램 폴더에 두세요.\n2. 아래 버튼으로 명령을 설정하세요.\n3. 터미널이나 에이전트를 다시 열어 주세요.", frame: NSRect(x: 32, y: 156, width: 536, height: 92), size: 16)
        status = label("기존 명령과 셸 설정은 변경 전에 백업합니다.", frame: NSRect(x: 32, y: 80, width: 536, height: 65), size: 14)
        installButton = NSButton(title: "명령 사용 설정", target: self, action: #selector(installCommands))
        installButton.bezelStyle = .rounded; installButton.frame = NSRect(x: 382, y: 25, width: 186, height: 40)
        view.addSubview(installButton)
        let permissions = NSButton(title: "화면 녹화 권한 안내", target: self, action: #selector(showPermissions))
        permissions.bezelStyle = .rounded; permissions.frame = NSRect(x: 32, y: 25, width: 196, height: 40)
        view.addSubview(permissions)
        window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
    }
    @objc func installCommands() {
        let bundle = Bundle.main.bundleURL.standardizedFileURL
        let userApplications = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications").path + "/"
        guard bundle.path.hasPrefix("/Applications/") || bundle.path.hasPrefix(userApplications) else {
            status.stringValue = "앱을 응용 프로그램 폴더로 옮긴 뒤 다시 열어 주세요. 디스크 이미지 안에서는 명령을 설정할 수 없습니다."
            return
        }
        do {
            try CommandSetup(home: FileManager.default.homeDirectoryForCurrentUser, bundle: bundle).install()
            status.stringValue = "설정 완료. 터미널·에이전트를 다시 열면 rec를 사용할 수 있습니다.\n에이전트에게 “rec로 작업을 녹화해”라고 요청하세요."
            installButton.title = "설정 완료"
        } catch { status.stringValue = error.localizedDescription }
    }
    @objc func showPermissions() {
        let alert = NSAlert()
        alert.messageText = "녹화를 실행하는 앱에 권한을 허용하세요"
        alert.informativeText = "시스템 설정 → 개인정보 보호 및 보안 → 화면 및 시스템 오디오 녹음에서 터미널 또는 에이전트 앱을 허용한 뒤 해당 앱을 다시 실행하세요. 권한은 자동으로 변경하지 않습니다."
        alert.addButton(withTitle: "시스템 설정 열기"); alert.addButton(withTitle: "닫기")
        if alert.runModal() == .alertFirstButtonReturn,
            let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture") { NSWorkspace.shared.open(url) }
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

// Exercise filesystem setup in an isolated directory without launching the UI.
if CommandLine.arguments.count == 4 && CommandLine.arguments[1] == "--check-setup" {
    do {
        try CommandSetup(home: URL(fileURLWithPath: CommandLine.arguments[2]), bundle: URL(fileURLWithPath: CommandLine.arguments[3])).install()
        print("Command setup complete")
    } catch { fputs("\(error.localizedDescription)\n", stderr); exit(1) }
} else {
    let app = NSApplication.shared
    let delegate = SetupDelegate()
    app.setActivationPolicy(.regular); app.delegate = delegate; app.run()
}
