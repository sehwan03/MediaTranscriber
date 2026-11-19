//! Windows 실행 파일에 아이콘과 버전 정보를 넣는다.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico")
        .set("ProductName", "MediaTranscriber")
        .set("FileDescription", "MediaTranscriber - 음성·동영상 텍스트 추출")
        .set("OriginalFilename", "MediaTranscriber.exe")
        .set("InternalName", "MediaTranscriber");
    if let Err(err) = res.compile() {
        // 리소스 컴파일러(rc.exe)가 없는 환경에서도 빌드는 계속한다 (아이콘만 빠짐).
        println!("cargo:warning=Windows 리소스(아이콘/버전) 포함 실패: {err}");
    }
}
