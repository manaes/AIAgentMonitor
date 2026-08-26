//! mDNS 게시와, 손으로 넣을 LAN 주소.
//!
//! CYD 는 `_aim._tcp.local.` 을 브라우즈해 맥을 찾는다. 조회가 막힌 망(mDNS 를
//! 걸러 내는 AP, 게스트 VLAN)도 있으므로 맥 UI 는 IP 를 함께 보여준다 — 사람이
//! 기기에 직접 넣는 대체 경로다.
//!
//! **광고에는 비밀이 없다.** 싣는 것은 포트와 프로토콜 세대뿐이고, 페어링 코드나
//! 토큰에서 나온 값은 하나도 넣지 않는다. mDNS 는 링크 전체에 뿌려지므로 여기
//! 실은 것은 같은 WiFi 의 누구나 읽는다. 광고가 말하는 것은 "여기에 서비스가
//! 있다"까지이고, 그 뒤로 한 바이트라도 더 받으려면 `AUTH2`/`PROOF2` 를 통과해야
//! 한다(`server::upgrade`).
//!
//! **광고는 리스너가 실제로 떠 있는 동안에만 나간다.** 그 판단은 여기가 아니라
//! `LanBridge::advertise` 에 있다 — 이 모듈은 시키는 대로 게시하고 거둘 뿐이다.
//!
//! **실패는 두 통로로 나온다.** `publish` 가 돌려주는 `Err` 는 게시를 **시작조차**
//! 못 한 경우고, 시작한 뒤에 드러나는 실패(멀티캐스트 차단 등)는 `ErrorSink` 로
//! 온다. 둘을 하나로 뭉치지 않는 이유는 `ErrorSink` 의 doc 에 적었다.

use mdns_sd::{DaemonEvent, ServiceDaemon, ServiceInfo};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

/// CYD 펌웨어가 브라우즈하는 서비스 타입.
///
/// **언어를 건너뛰는 계약이다.** 이 문자열은 Rust 쪽에만 있는 것이 아니라 ESP32
/// 펌웨어에도 같은 리터럴로 박혀 있다. 여기서 한 글자를 바꾸면 컴파일도 테스트도
/// 통과하는데 하드웨어만 조용히 맥을 찾지 못한다 — 아래 `service_type_matches_spec`
/// 이 그 한 글자를 잡으라고 있는 테스트다.
pub const SERVICE_TYPE: &str = "_aim._tcp.local.";

/// TXT 에 싣는 프로토콜 세대. E2EE v2 를 말한다 — 기기가 붙기 전에 세대를 알면
/// 맞지 않는 펌웨어가 헛되이 핸드셰이크를 시작하지 않는다. 비밀이 아니다.
const PROTOCOL_GENERATION: &str = "2";

/// 호스트 이름을 하나도 못 건졌을 때 쓸 라벨. 같은 망에 이런 맥이 둘이면 이름이
/// 겹치지만, 겹치는 쪽은 mDNS 가 인스턴스 이름에 접미사를 붙여 갈라 준다.
const FALLBACK_LABEL: &str = "aim";

/// DNS 라벨 하나의 길이 상한(RFC 1035).
const MAX_LABEL_LEN: usize = 63;

/// 첫 goodbye 가 나갔다는 확인을 기다리는 상한. 넘기면 그냥 진행한다 — 확인을
/// 못 받았다고 데몬을 붙잡고 있을 이유는 없다.
const UNREGISTER_ACK_WAIT: Duration = Duration::from_millis(300);

/// 첫 goodbye 뒤에 데몬을 살려 두는 시간. `mdns-sd` 는 goodbye 를 **120ms 뒤에 한
/// 번 더** 보내도록 예약한다("repeat for one time just in case some peers miss the
/// message"). 곧바로 `shutdown` 하면 데몬이 그 타이머 전에 끝나 재전송이
/// 취소된다 — 멀티캐스트 UDP 한 장이 유실되면 CYD 는 TTL 이 만료될 때까지 죽은
/// 포트로 재접속을 시도한다. 120ms 에 여유를 더한 값이다.
const GOODBYE_RESEND_WAIT: Duration = Duration::from_millis(250);

/// 게시가 **시작한 뒤에** 실패했음을 알리는 통로.
///
/// 이것이 필요한 이유는 `ServiceDaemon::new()` 가 멀티캐스트 소켓을 열지 않기
/// 때문이다(크레이트 doc: 소켓은 데몬 스레드가 뜬 뒤 lazily 열리고, "multicast not
/// permitted" 같은 플랫폼 문제는 생성자가 아니라 나중에 `monitor()` 의
/// `DaemonEvent` 로 나온다). 즉 **실전에서 가장 흔한 실패**(방화벽이 5353 을 막았다,
/// 게스트 VLAN, 멀티캐스트 금지)는 `publish()` 의 반환값에 절대 나타나지 않는다.
/// 그 실패를 삼키면 사용자는 "켰는데 기기가 못 찾는다"만 겪고, IP 를 손으로 넣으라는
/// 안내를 **그 안내가 존재하는 이유인 바로 그 상황에서** 받지 못한다.
///
/// 콜백인 이유는 이 모듈이 `ServerEvent` 를 알 필요가 없기 때문이다. 무엇을 할지는
/// 부르는 쪽(`LanBridge::advertise`)이 정한다.
pub type ErrorSink = Box<dyn Fn(String) + Send + 'static>;

/// 살아 있는 게시 하나.
///
/// 그냥 떨어뜨려도 데몬 스레드는 끝나지만 **goodbye 는 나가지 않는다** — 조회하는
/// 쪽은 TTL 이 만료될 때까지 이 서비스가 살아 있다고 믿는다. 그래서 거두는 길은
/// `stop()` 이고, 소유자(`MdnsAdvertiser`)가 `Drop` 에서도 그것을 부른다.
pub struct Publication {
    daemon: ServiceDaemon,
    /// `unregister` 가 요구하는 전체 이름(`AIM-호스트._aim._tcp.local.`).
    /// 등록한 뒤에 데몬이 붙인 접미사가 반영된 값이라 우리가 다시 만들지 않는다.
    fullname: String,
}

impl Publication {
    /// 광고를 거둔다. `unregister` 는 goodbye 패킷을 뿌려 상대가 TTL(수 분)을
    /// 기다리지 않고 바로 잊게 한다 — 그것 없이 데몬만 내리면, 토글을 끈 뒤에도
    /// CYD 는 한참 동안 죽은 포트를 향해 재접속을 시도한다.
    /// **기다리는 부분만 따로 스레드로 옮긴다.** 이 함수는 토글을 내리는 동기
    /// 경로(`set_enabled(false)` → `LanBridge::advertise`)에서 불리므로 여기서
    /// 기다리면 UI 를 돌리는 스레드가 망 데몬을 기다리게 된다. 그렇다고 곧바로
    /// `shutdown` 을 던지면 `GOODBYE_RESEND_WAIT` 가 설명하는 재전송이 취소된다.
    /// 둘 다 피하는 길은 "부르는 쪽은 즉시 돌아가고, 기다림은 다른 스레드에서"다.
    ///
    /// 순서 자체는 데몬이 보장한다 — 명령이 하나의 FIFO 를 타므로 `unregister` 가
    /// goodbye 를 소켓에 쓴 뒤에야 `shutdown` 이 처리된다. 여기서 확인을 기다리는
    /// 것은 순서 때문이 아니라 **재전송 타이머의 기준점을 잡기 위해서**다: 확인이
    /// 온 시점이 첫 goodbye 가 나간 시점이고, 재전송은 거기서 120ms 뒤다.
    pub fn stop(self) {
        let Publication { daemon, fullname } = self;
        // 스레드를 못 띄웠을 때 쓸 손잡이. **핸들을 그냥 떨어뜨리면 데몬 스레드가
        // 멈추지 않는다** — 명령 채널이 끊겨도 루프는 계속 돈다(크레이트 구현).
        // 그러니 어느 갈래로 가든 `shutdown` 은 반드시 불려야 한다.
        let fallback = daemon.clone();
        let teardown = move || {
            if let Ok(ack) = daemon.unregister(&fullname) {
                let _ = ack.recv_timeout(UNREGISTER_ACK_WAIT);
            }
            std::thread::sleep(GOODBYE_RESEND_WAIT);
            let _ = daemon.shutdown();
        };
        if let Err(e) = std::thread::Builder::new()
            .name("aim-mdns-stop".to_string())
            .spawn(teardown)
        {
            // 사실상 자원 고갈이다. 재전송은 포기하고 데몬만 내린다 — 광고가
            // 남는 것보다 goodbye 가 한 번만 나가는 편이 낫다. `Exit` 는
            // 남아 있는 서비스에 대해 goodbye 를 뿌리고 끝난다(크레이트 `cleanup`).
            tracing::warn!("mDNS 정리 스레드를 띄우지 못했다: {e}");
            let _ = fallback.shutdown();
        }
    }
}

/// 광고를 실제로 수행하는 것.
///
/// 트레이트로 둔 이유는 **유닛 테스트가 진짜 mDNS 데몬을 띄우면 안 되기**
/// 때문이다. 데몬은 스레드와 멀티캐스트 소켓을 잡는데, 그보다 나쁜 것은 `cargo
/// test` 가 개발자의 망에 이 맥을 **실제로 광고한다**는 점이다. 정작 테스트가 봐야
/// 하는 것은 데몬이 아니라 "언제 켜고 언제 끄는가"라는 판단이고, 그 판단은
/// `LanBridge` 에 있다. 그래서 판단은 그대로 두고 효과만 갈아 끼운다.
pub trait Advertiser: Send {
    /// 이 포트를 광고한다. 부르는 쪽이 이미 광고 중인지를 안다
    /// (`LanBridge::advertise`) — 여기서 다시 세지 않는다.
    ///
    /// **`Ok(())` 는 "광고가 망에 닿는다"가 아니라 "게시를 시작했다"는 뜻이다.**
    /// 그 뒤에 드러나는 실패는 `on_error` 로 온다(`ErrorSink` 의 doc). 두 통로가
    /// 나뉘어 있는 것이 이 시그니처의 요점이다 — 돌려주는 값 하나로 뭉치면
    /// 비동기 실패가 갈 곳이 없어지고, 그게 곧 조용한 실패다.
    fn start(&mut self, port: u16, on_error: ErrorSink) -> anyhow::Result<()>;
    /// 광고를 거둔다. 광고 중이 아니면 아무 일도 일어나지 않는다.
    fn stop(&mut self);
}

/// 진짜 mDNS 데몬. 운영에서 `LanBridge` 가 드는 것이 이것이다.
#[derive(Default)]
pub struct MdnsAdvertiser {
    live: Option<Publication>,
}

impl Advertiser for MdnsAdvertiser {
    fn start(&mut self, port: u16, on_error: ErrorSink) -> anyhow::Result<()> {
        // 앞선 게시가 남아 있으면 먼저 거둔다. 두 개를 동시에 등록하면 데몬이
        // 인스턴스 이름에 접미사를 붙여 갈라 놓고, 기기 목록에 유령이 하나 는다.
        self.stop();
        self.live = Some(publish(port, on_error)?);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(p) = self.live.take() {
            p.stop();
        }
    }
}

impl Drop for MdnsAdvertiser {
    /// 앱이 끝날 때도 goodbye 를 뿌린다. `LanBridge` 가 통째로 사라지는 경로
    /// (앱 종료)에는 `advertise(false)` 를 불러 줄 사람이 없다.
    fn drop(&mut self) {
        self.stop();
    }
}

/// 서비스 하나를 게시한다.
///
/// `port` 를 인자로 받는 이유는 **광고한 포트와 실제로 열린 포트가 같아야**
/// 하기 때문이다. `server::PORT` 를 여기서 다시 읽으면 둘이 갈라질 여지가
/// 생긴다 — 부르는 쪽(`LanBridge`)은 자기가 bind 한 포트를 알고 있다.
///
/// 주소는 `enable_addr_auto()` 로 데몬이 채운다. 우리가 고른 주소 하나를 박아
/// 두면 WiFi 와 유선을 오갈 때 광고가 옛 주소를 계속 말한다.
pub fn publish(port: u16, on_error: ErrorSink) -> anyhow::Result<Publication> {
    let daemon = ServiceDaemon::new()?;
    // **등록보다 먼저 건다.** 데몬 스레드는 이미 돌기 시작했고 소켓을 여는 것도
    // 그쪽이다 — 등록 뒤에 걸면 그 사이에 나온 실패를 놓친다.
    let events = daemon.monitor()?;
    let info = build_service_info(port)?;
    let fullname = info.get_fullname().to_string();
    daemon.register(info)?;
    tracing::info!(port, fullname = %fullname, "mDNS 게시");

    // 데몬의 통지를 읽는 스레드. `tokio::spawn` 이 아닌 이유는 이 함수가 런타임
    // 밖에서도 불릴 수 있어야 하기 때문이다(브리지의 `advertise` 는 동기다).
    // 데몬이 내려가면 채널이 닫히고 이 스레드는 스스로 끝난다 — 그래서 join 할
    // 핸들을 들고 있지 않는다.
    std::thread::Builder::new()
        .name("aim-mdns-monitor".to_string())
        .spawn(move || {
            while let Ok(ev) = events.recv() {
                // `Error` 만 사용자에게 올린다. `IpAdd`/`IpDel`/`Respond` 는 정상
                // 동작의 소음이고, `NameChange`(이름 충돌로 접미사가 붙었다)는
                // 실패가 아니다 — CYD 는 인스턴스 이름이 아니라 타입을
                // 브라우즈하므로 그래도 찾는다.
                if let DaemonEvent::Error(e) = ev {
                    tracing::warn!("mDNS 데몬 오류: {e}");
                    on_error(e.to_string());
                }
            }
        })?;

    Ok(Publication { daemon, fullname })
}

/// 게시할 레코드를 만든다. **데몬과 떼어 둔 이유는 테스트다** — 무엇이 광고에
/// 실리는지는 망을 건드리지 않고 확인할 수 있어야 한다.
fn build_service_info(port: u16) -> anyhow::Result<ServiceInfo> {
    let label = host_label();
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        // 인스턴스 이름은 사람이 기기 목록에서 자기 맥을 알아보는 이름이다.
        &format!("AIM-{label}"),
        &format!("{label}.local."),
        (),
        port,
        &[("v", PROTOCOL_GENERATION)][..],
    )?
    .enable_addr_auto();
    Ok(info)
}

/// mDNS 레코드에 쓸 호스트 라벨.
///
/// `scutil --get LocalHostName` 이 아니다 — 그것은 맥에만 있고, 이 앱은 윈도우
/// MSI 도 낸다(`docs/release-notes/v1.5.0.md`). 윈도우에서 그 서브프로세스는 그냥
/// 실패해 모든 맥이 `mac.local.` 이라는 같은 이름을 주장하게 된다.
fn host_label() -> String {
    sanitize_label(&gethostname::gethostname().to_string_lossy())
}

/// 아무 컴퓨터 이름이나 DNS 라벨 하나로 줄인다.
///
/// 세 가지를 고친다:
/// - **점**: 맥의 `gethostname(2)` 은 흔히 `Wanny-MBP.local` 을 준다. 그대로 쓰면
///   `Wanny-MBP.local.local.` 이 된다. 첫 마디만 쓴다.
/// - **라벨에 못 쓰는 글자**: 호스트 이름은 영숫자와 하이픈이다(RFC 1123). 한국에서
///   기본 컴퓨터 이름은 `완영의 MacBook Pro` 처럼 한글과 공백을 담는다 — 그대로
///   레코드에 넣으면 조회하는 쪽이 무엇을 받을지 우리가 보증할 수 없다.
/// - **길이**: 라벨은 63바이트까지다.
///
/// 남는 것이 없으면 `FALLBACK_LABEL` 이다. 이름 전체가 한글인 맥이 그 경우인데,
/// 흔한 일이라 무시하지 않고 이름을 준다.
fn sanitize_label(raw: &str) -> String {
    let head = raw.split('.').next().unwrap_or("");
    let mut out = String::with_capacity(head.len());
    for ch in head.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            out.push(ch);
        } else if !out.ends_with('-') {
            // 못 쓰는 글자는 하이픈 하나로 접는다. 이어 붙이면 `---` 가 남는다.
            out.push('-');
        }
    }
    let trimmed: String = out.trim_matches('-').chars().take(MAX_LABEL_LEN).collect();
    // 자르고 나서 끝이 하이픈이 될 수 있다.
    let trimmed = trimmed.trim_end_matches('-');
    if trimmed.is_empty() {
        FALLBACK_LABEL.to_string()
    } else {
        trimmed.to_string()
    }
}

/// 라우팅 테이블에 길을 물을 때 쓰는 목적지. RFC 5737 의 문서용 주소
/// (TEST-NET-1)다.
///
/// 일부러 인터넷에 라우팅되지 않는 주소를 골랐다. `8.8.8.8` 처럼 실재하는 호스트를
/// 적으면 이 코드를 읽는 다음 사람이 "인터넷이 있어야 동작한다"로 읽고, 실제로는
/// 아무 패킷도 나가지 않는데도 그 오해가 오래 남는다.
const ROUTE_PROBE: (Ipv4Addr, u16) = (Ipv4Addr::new(192, 0, 2, 1), 9);

/// 사용자에게 보여줄 LAN IPv4. 없으면 `None`.
///
/// **서브프로세스를 쓰지 않는다.** `ipconfig getifaddr en0` 은 맥에서만 그 뜻이다.
/// 윈도우에도 `ipconfig` 가 있지만 인자가 전혀 달라 설정 덤프 전체가 stdout 으로
/// 쏟아지고, 그 쓰레기는 `"127."` 로 시작하지 않으므로 루프백 검사를 그대로
/// 통과해 **사용자 화면에 "당신의 LAN 주소"로 뜬다**. 이 앱은 윈도우 MSI 도 낸다.
///
/// 대신 커널에 직접 묻는다: UDP 소켓을 `0.0.0.0:0` 에 묶고 바깥 주소로 `connect`
/// 한 다음 `local_addr()` 을 읽는다. UDP 의 `connect` 는 **패킷을 보내지 않는다** —
/// 목적지를 고정해 커널이 경로를 고르게 할 뿐이고, 그 경로가 곧 송신 인터페이스와
/// 그 인터페이스의 주소다. 그래서 인터페이스 이름을 한 글자도 적지 않고 기본
/// 경로의 주소를 얻는다. `en0`/`en1` 을 적어 두는 쪽은 유선을 쓰는 사용자에게
/// 그냥 틀린 답을 준다(이 맥만 해도 `en1`~`en6` 이 브리지·VM 으로 올라와 있다).
///
/// **한계 두 가지.** 전 구간(full-tunnel) VPN 이 기본 경로를 가져가면 여기서 나오는
/// 것은 터널 주소이고, CYD 는 그 주소로 붙지 못한다. 기본 경로가 아예 없으면
/// `connect` 가 실패해 `None` 이다 — 랜선이 빠진 상태가 그렇다.
pub fn local_ipv4() -> Option<String> {
    // 0.0.0.0 에 묶으므로 돌아오는 주소는 언제나 IPv4 다.
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    sock.connect(ROUTE_PROBE).ok()?;
    let SocketAddr::V4(local) = sock.local_addr().ok()? else {
        return None;
    };
    let ip = *local.ip();
    // 사람이 CYD 에 넣어도 붙지 못하는 주소는 보여주지 않는다. 보여주면 사용자는
    // 자기가 잘못 넣은 줄 안다.
    if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() {
        return None;
    }
    Some(ip.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 상수를 자기 리터럴과 비교한다 — 보통은 아무것도 증명하지 못하는 형태다.
    /// **여기서만 예외인 이유**: 이 문자열은 우리 코드에만 있는 것이 아니라 ESP32
    /// 펌웨어에 같은 리터럴로 박혀 있는 **언어를 건너뛰는 계약**이다. Rust 쪽에서
    /// 이름을 바꾸면 이 저장소의 무엇도 깨지지 않고 하드웨어만 조용히 맥을 찾지
    /// 못하게 된다. 이 테스트가 그 변경을 여기서 멈춰 세운다. 이유를 지우면 다음
    /// 사람이 테스트를 지운다.
    #[test]
    fn service_type_matches_spec() {
        assert_eq!(SERVICE_TYPE, "_aim._tcp.local.");
    }

    /// 수동 입력 대비로 맥 UI 가 IP 를 보여준다. 루프백을 고르면 사용자가 그
    /// 값을 CYD 에 넣어도 붙지 못한다.
    ///
    /// **한계 — 이 테스트는 망이 없으면 비어서 통과한다.** `None` 이면 단정할 것이
    /// 없다. 그것을 승인이 아니라 한계로 적어 둔다: CI 에 실제 LAN 을 요구하는
    /// 쪽이 더 나쁘기 때문에 받아들이는 것이고, 값이 나왔을 때는 **반드시** 값을
    /// 검사한다. 실제로 광고된 주소로 기기가 붙는지는 손으로만 확인된다
    /// (`docs/ble-protocol/DEVICE-TEST.md`).
    #[test]
    fn local_ipv4_is_a_reachable_looking_address() {
        let Some(ip) = local_ipv4() else {
            return;
        };
        let parsed: Ipv4Addr = ip
            .parse()
            .unwrap_or_else(|e| panic!("IPv4 로 파싱되지 않는 값을 보여주려 한다: {ip:?} ({e})"));
        assert!(!parsed.is_loopback(), "루프백을 고르면 안 된다: {ip}");
        assert!(!parsed.is_link_local(), "링크로컬은 붙지 못한다: {ip}");
        assert!(!parsed.is_unspecified(), "0.0.0.0 은 주소가 아니다: {ip}");
    }

    /// 맥의 `gethostname(2)` 은 흔히 `.local` 이 붙은 이름을 준다. 그대로 쓰면
    /// 레코드가 `...local.local.` 이 된다.
    #[test]
    fn a_host_label_keeps_only_the_first_dotted_part() {
        assert_eq!(sanitize_label("Wanny-MBP.local"), "Wanny-MBP");
        assert_eq!(sanitize_label("desk.example.com"), "desk");
    }

    /// 한국에서 기본 컴퓨터 이름은 한글과 공백을 담는다. 라벨에는 영숫자와
    /// 하이픈만 남아야 하고, 접힌 하이픈이 줄줄이 남거나 끝에 매달려도 안 된다.
    #[test]
    fn a_host_label_is_a_dns_label() {
        assert_eq!(sanitize_label("완영의 MacBook Pro"), "MacBook-Pro");
        assert_eq!(sanitize_label("my_mac (2)"), "my-mac-2");
        assert!(
            sanitize_label("완영의 MacBook Pro")
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "라벨에 쓸 수 없는 글자가 남았다"
        );
    }

    /// 이름이 통째로 한글이면 남는 글자가 없다. 빈 라벨은
    /// `.local.` 하나짜리 이름이 되어 레코드를 망가뜨리므로 대체 이름을 준다.
    #[test]
    fn a_host_label_is_never_empty() {
        assert_eq!(sanitize_label("완영의맥"), FALLBACK_LABEL);
        assert_eq!(sanitize_label(""), FALLBACK_LABEL);
        assert_eq!(sanitize_label("---"), FALLBACK_LABEL);
    }

    /// 라벨은 63바이트까지다(RFC 1035). 잘린 끝이 하이픈으로 끝나서도 안 된다.
    #[test]
    fn a_host_label_fits_in_one_dns_label() {
        let long = "a".repeat(200);
        assert_eq!(sanitize_label(&long).len(), MAX_LABEL_LEN);

        // **자른 자리에 하이픈이 오도록** 만든다: 63번째 글자가 접힌 하이픈이 되고
        // 64번째부터 잘려 나간다. 하이픈으로 끝나는 라벨은 DNS 이름이 아니므로
        // 자른 **뒤에** 한 번 더 다듬어야 한다.
        let awkward = format!("{}!x", "b".repeat(MAX_LABEL_LEN - 1));
        assert_eq!(sanitize_label(&awkward), "b".repeat(MAX_LABEL_LEN - 1));
    }

    /// 진짜 데몬으로 한 바퀴 돈다 — 게시하고, 다른 데몬으로 브라우즈해서 우리
    /// 레코드를 찾는다.
    ///
    /// **평소 실행에서 빠져 있는 이유**는 이것이 개발자의 망에 이 맥을 실제로
    /// 광고하고, 멀티캐스트가 되는 환경을 요구하기 때문이다:
    ///
    /// ```text
    /// cargo test --manifest-path src-tauri/Cargo.toml lan::discovery -- --ignored --nocapture
    /// ```
    ///
    /// 이것이 덮는 층은 위쪽 테스트들이 닿지 못하는 곳이다: `enable_addr_auto` 가
    /// 주소를 실제로 채우는가, 라벨이 레코드에서 살아남는가, TXT 가 그대로 실려
    /// 가는가, 그리고 **`ErrorSink` 가 조용한가**(데몬이 오류를 냈다면 여기 담긴다).
    ///
    /// 개발 중인 앱이 같은 맥에서 이미 광고 중이면 데몬이 인스턴스 이름에 접미사를
    /// 붙여 갈라 놓는다 — 아래 단정(포트·TXT·주소)은 어느 쪽을 찾든 참이다.
    #[test]
    #[ignore = "진짜 mDNS 데몬을 띄우고 이 맥을 망에 광고한다 — 손으로만"]
    fn a_real_daemon_publishes_a_record_that_can_be_found() {
        use mdns_sd::ServiceEvent;
        use std::sync::{Arc, Mutex};
        use std::time::Instant;

        let errors: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink_errors = errors.clone();
        let published = publish(
            super::super::server::PORT,
            Box::new(move |e| sink_errors.lock().unwrap().push(e)),
        )
        .expect("게시하지 못했다");

        let seeker = ServiceDaemon::new().expect("브라우즈용 데몬");
        let found = seeker.browse(SERVICE_TYPE).expect("브라우즈 시작");

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline
                .checked_duration_since(Instant::now())
                .expect("10초 안에 우리 레코드를 찾지 못했다");
            match found.recv_timeout(left) {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    assert_eq!(info.get_port(), super::super::server::PORT);
                    assert!(
                        !info.get_addresses().is_empty(),
                        "enable_addr_auto 가 주소를 채우지 못했다 — 기기가 갈 곳이 없다"
                    );
                    assert_eq!(
                        info.get_property_val_str("v"),
                        Some(PROTOCOL_GENERATION),
                        "TXT 가 레코드까지 살아남아야 한다"
                    );
                    break;
                }
                Ok(_) => continue,
                Err(e) => panic!("브라우즈 통로가 끝났다: {e}"),
            }
        }

        let _ = seeker.shutdown();
        published.stop();
        assert_eq!(
            *errors.lock().unwrap(),
            Vec::<String>::new(),
            "데몬이 오류를 냈다 — 이 망에서는 게시가 조용히 실패한다"
        );
    }

    /// 진짜 이름에서 나오는 라벨도 비어 있으면 안 된다. 여기서 데몬은 띄우지
    /// 않는다 — 게시 자체는 위의 `--ignored` 테스트와 DEVICE-TEST §8 이 본다.
    #[test]
    fn the_machine_has_a_usable_label() {
        let label = host_label();
        assert!(!label.is_empty());
        assert!(!label.contains('.'), "라벨에 점이 있으면 안 된다: {label}");
        assert!(label.len() <= MAX_LABEL_LEN);
    }

    /// 레코드를 실제로 만들어 무엇이 실리는지 본다. `ServiceInfo::new` 는 데몬을
    /// 필요로 하지 않으므로 망을 건드리지 않고 확인할 수 있다.
    ///
    /// **TXT 에는 프로토콜 세대 하나뿐이어야 한다.** mDNS 는 링크 전체에 뿌려지므로
    /// 여기 섞인 것은 같은 WiFi 의 누구나 읽는다 — 페어링 코드나 토큰에서 나온
    /// 값이 하나라도 들어오면 그 순간 전송의 전제가 무너진다.
    #[test]
    fn the_record_advertises_the_port_and_nothing_secret() {
        let info = build_service_info(4320).expect("레코드를 만들지 못했다");

        assert_eq!(info.get_port(), 4320, "광고한 포트가 실제 포트여야 한다");
        assert!(
            info.get_fullname().ends_with(SERVICE_TYPE),
            "브라우즈되는 타입 아래에 있어야 한다: {}",
            info.get_fullname()
        );

        let props: Vec<(String, String)> = info
            .get_properties()
            .iter()
            .map(|p| (p.key().to_string(), p.val_str().to_string()))
            .collect();
        assert_eq!(
            props,
            vec![("v".to_string(), PROTOCOL_GENERATION.to_string())],
            "TXT 에는 프로토콜 세대만 실린다"
        );
    }
}
