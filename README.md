# Connected

macOS, Windows, Linux 컴퓨터끼리 키보드와 마우스를 나눠 쓰는 프로그램입니다. 마우스를 화면 가장자리 밖으로 밀면 옆 컴퓨터로 넘어갑니다.

설계와 진행 상황은 [DESIGN.md](DESIGN.md)에 있습니다.

## 만들기

Rust(stable)와 [bun](https://bun.sh)이 필요합니다.

```sh
bun install
bun run test      # 화면 로직 테스트
cargo test --workspace
bun run build     # macOS: target/release/bundle/macos/Connected.app
```

Windows 설치 파일은 `bun run --cwd crates/connected-desktop tauri build --bundles nsis`로 만듭니다.

GitHub Actions에서 빌드됩니다.

- **Build Windows** (`.github/workflows/build-windows.yml`): GitHub 서버(`windows-latest`)에서 설치 파일과 단독 실행 파일을 만듭니다.
- **Build macOS** (`.github/workflows/build-macos.yml`): GitHub 서버(`macos-latest`)에서 `.app`과 `.dmg`를 만듭니다.

둘 다 Actions 탭에서 직접 실행하거나, `v*` 태그를 푸시하면(`test-*`는 시험판) Release에 파일이 붙습니다.
