//! 백프레셔 송신 큐 (스펙 4.5).
//!
//! CoreBluetooth 의 updateValue 는 전송 큐가 가득 차면 false 를 반환하고 **그 청크를 버린다**.
//! 반환값을 무시하면 프레임 중간 청크가 조용히 사라져 수신 측이 영원히 프레임을 완성하지 못한다.
//!
//! 정책: 최신값 우선. 아직 한 청크도 보내지 않은 프레임은 새 프레임으로 통째 교체하고,
//! 이미 일부를 보낸 프레임은 끝까지 보낸 뒤 교체한다(수신 측 부분 프레임 폐기 비용 절감).
use std::collections::VecDeque;

#[derive(Debug, Default)]
pub struct SendQueue {
    current: VecDeque<Vec<u8>>,
    /// current 의 청크를 하나라도 실제로 보냈는지
    started: bool,
    /// 다음에 보낼 최신 프레임. 새 offer 가 오면 통째로 덮어쓴다.
    next: Option<Vec<Vec<u8>>>,
    paused: bool,
}

impl SendQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn offer(&mut self, chunks: Vec<Vec<u8>>) {
        if self.started && !self.current.is_empty() {
            // 진행 중인 프레임은 건드리지 않고, 대기 슬롯만 최신으로 교체한다.
            self.next = Some(chunks);
        } else {
            self.current = chunks.into();
            self.started = false;
            self.next = None;
        }
    }

    /// `send` 는 성공 시 true, 전송 큐 포화 시 false 를 반환해야 한다.
    pub fn pump(&mut self, mut send: impl FnMut(&[u8]) -> bool) {
        loop {
            if self.current.is_empty() {
                match self.next.take() {
                    Some(n) => {
                        self.current = n.into();
                        self.started = false;
                    }
                    None => return,
                }
            }
            if self.paused {
                return;
            }
            let Some(front) = self.current.front() else {
                return;
            };
            if send(front) {
                self.current.pop_front();
                self.started = true;
            } else {
                self.paused = true;
                return;
            }
        }
    }

    /// peripheralManagerIsReadyToUpdateSubscribers: 수신 시 호출한다.
    pub fn on_ready(&mut self) {
        self.paused = false;
    }

    pub fn is_idle(&self) -> bool {
        self.current.is_empty() && self.next.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// n번째 호출까지만 성공하고 이후 false 를 돌려주는 가짜 송신기
    fn limited(limit: usize) -> (impl FnMut(&[u8]) -> bool, std::rc::Rc<std::cell::RefCell<Vec<Vec<u8>>>>) {
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let s = sent.clone();
        let f = move |c: &[u8]| {
            if s.borrow().len() >= limit {
                return false;
            }
            s.borrow_mut().push(c.to_vec());
            true
        };
        (f, sent)
    }

    #[test]
    fn sends_all_chunks_when_not_saturated() {
        let mut q = SendQueue::new();
        q.offer(vec![vec![1], vec![2], vec![3]]);
        let (mut send, sent) = limited(100);
        q.pump(&mut send);
        assert_eq!(sent.borrow().len(), 3);
        assert!(q.is_idle());
    }

    #[test]
    fn pauses_on_saturation_and_resumes_on_ready() {
        let mut q = SendQueue::new();
        q.offer(vec![vec![1], vec![2], vec![3]]);
        let (mut send, sent) = limited(2);
        q.pump(&mut send);
        assert_eq!(sent.borrow().len(), 2, "2개까지만 나가고 멈춘다");
        assert!(!q.is_idle());

        // 포화 상태에서 다시 pump 해도 나가지 않아야 한다
        q.pump(&mut send);
        assert_eq!(sent.borrow().len(), 2);

        // 큐가 비워졌다는 신호가 오면 재개
        let (mut send2, sent2) = limited(100);
        q.on_ready();
        q.pump(&mut send2);
        assert_eq!(sent2.borrow().len(), 1, "남은 1개가 나간다");
        assert!(q.is_idle());
    }

    #[test]
    fn replaces_untouched_frame_with_latest() {
        let mut q = SendQueue::new();
        q.offer(vec![vec![1], vec![2]]);
        q.offer(vec![vec![9]]); // 아직 한 청크도 안 보냈으므로 통째로 교체
        let (mut send, sent) = limited(100);
        q.pump(&mut send);
        assert_eq!(*sent.borrow(), vec![vec![9u8]], "오래된 프레임은 버린다");
    }

    #[test]
    fn finishes_started_frame_before_switching() {
        let mut q = SendQueue::new();
        q.offer(vec![vec![1], vec![2], vec![3]]);
        let (mut send, sent) = limited(1);
        q.pump(&mut send); // 1개 전송 후 포화 → 이 프레임은 "시작됨"
        assert_eq!(sent.borrow().len(), 1);

        q.offer(vec![vec![9]]); // 진행 중 프레임은 끝까지 보낸 뒤 교체
        let (mut send2, sent2) = limited(100);
        q.on_ready();
        q.pump(&mut send2);
        assert_eq!(
            *sent2.borrow(),
            vec![vec![2u8], vec![3u8], vec![9u8]],
            "시작한 프레임을 마친 뒤 최신 프레임을 보낸다"
        );
    }

    #[test]
    fn only_latest_pending_frame_is_kept() {
        let mut q = SendQueue::new();
        q.offer(vec![vec![1], vec![2]]);
        let (mut send, _sent) = limited(1);
        q.pump(&mut send); // 시작됨

        q.offer(vec![vec![7]]);
        q.offer(vec![vec![8]]); // 7 은 버려지고 8 만 남는다

        let (mut send2, sent2) = limited(100);
        q.on_ready();
        q.pump(&mut send2);
        assert_eq!(*sent2.borrow(), vec![vec![2u8], vec![8u8]]);
    }
}
