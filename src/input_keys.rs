#![allow(dead_code)]

use evdev::KeyCode;
use serde::{Deserialize, Serialize};

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_MSC: u16 = 0x04;

const SYN_REPORT: u16 = 0x00;
const MSC_SCAN: u16 = 0x04;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawInputEvent {
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}

impl RawInputEvent {
    pub const fn new(event_type: u16, code: u16, value: i32) -> Self {
        Self {
            event_type,
            code,
            value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
    Up,
    Down,
    Repeat,
}

impl KeyState {
    fn from_raw(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Up),
            1 => Some(Self::Down),
            2 => Some(Self::Repeat),
            _ => None,
        }
    }

    fn is_pressed(self) -> bool {
        !matches!(self, Self::Up)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardKeyEvent {
    pub linux_code: u16,
    pub key_name: String,
    pub state: KeyState,
    pub raw_scancode: Option<i32>,
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingKeyboardEvent {
    pub captured_at_ms: u64,
    pub device_ptr: Option<String>,
    pub key: KeyboardKeyEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardInputEvent {
    pub sequence: u64,
    pub captured_at_ms: u64,
    pub device_ptr: Option<String>,
    #[serde(flatten)]
    pub key: KeyboardKeyEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardEventBatch {
    pub published_at_ms: u64,
    pub dropped_events: u64,
    pub events: Vec<KeyboardInputEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyboardHistoryResponse {
    pub generated_at_ms: u64,
    pub retention_secs: u64,
    pub dropped_events: u64,
    pub events: Vec<KeyboardInputEvent>,
}

#[derive(Debug, Default)]
pub struct KeyboardEventDecoder {
    pending_scan: Option<i32>,
    modifiers: Modifiers,
}

impl KeyboardEventDecoder {
    pub fn decode_event(&mut self, event: RawInputEvent) -> Option<KeyboardKeyEvent> {
        match (event.event_type, event.code) {
            (EV_MSC, MSC_SCAN) => {
                self.pending_scan = Some(event.value);
                None
            }
            (EV_SYN, SYN_REPORT) => {
                self.pending_scan = None;
                None
            }
            (EV_KEY, code) => {
                let state = KeyState::from_raw(event.value)?;
                let key = KeyCode::new(code);

                self.update_modifiers(key, state);

                Some(KeyboardKeyEvent {
                    linux_code: code,
                    key_name: linux_key_name(code),
                    state,
                    raw_scancode: self.pending_scan.take(),
                    modifiers: self.modifiers.clone(),
                })
            }
            _ => None,
        }
    }

    pub fn decode_all<I>(&mut self, events: I) -> Vec<KeyboardKeyEvent>
    where
        I: IntoIterator<Item = RawInputEvent>,
    {
        events
            .into_iter()
            .filter_map(|event| self.decode_event(event))
            .collect()
    }

    fn update_modifiers(&mut self, key: KeyCode, state: KeyState) {
        let pressed = state.is_pressed();

        match key {
            KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL => self.modifiers.ctrl = pressed,
            KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT => self.modifiers.shift = pressed,
            KeyCode::KEY_LEFTALT | KeyCode::KEY_RIGHTALT => self.modifiers.alt = pressed,
            KeyCode::KEY_LEFTMETA | KeyCode::KEY_RIGHTMETA => self.modifiers.meta = pressed,
            _ => {}
        }
    }
}

pub fn linux_key_name(code: u16) -> String {
    format!("{:?}", KeyCode::new(code))
}

#[cfg(test)]
mod tests {
    use super::{
        EV_KEY, EV_MSC, EV_SYN, KeyState, KeyboardEventDecoder, MSC_SCAN, RawInputEvent,
        SYN_REPORT, linux_key_name,
    };

    #[test]
    fn maps_linux_key_codes_to_expected_symbolic_names() {
        let baseline = [
            (20_u16, "KEY_T"),
            (18_u16, "KEY_E"),
            (31_u16, "KEY_S"),
            (29_u16, "KEY_LEFTCTRL"),
            (46_u16, "KEY_C"),
        ];

        for (code, expected) in baseline {
            assert_eq!(linux_key_name(code), expected, "unexpected mapping for code {code}");
        }
    }

    #[test]
    fn decodes_test_then_ctrl_c_trace_against_baseline() {
        let trace = [
            RawInputEvent::new(EV_MSC, MSC_SCAN, 458775),
            RawInputEvent::new(EV_KEY, 20, 1),
            RawInputEvent::new(EV_SYN, SYN_REPORT, 0),
            RawInputEvent::new(EV_MSC, MSC_SCAN, 458775),
            RawInputEvent::new(EV_KEY, 20, 0),
            RawInputEvent::new(EV_SYN, SYN_REPORT, 0),
            RawInputEvent::new(EV_MSC, MSC_SCAN, 458760),
            RawInputEvent::new(EV_KEY, 18, 1),
            RawInputEvent::new(EV_SYN, SYN_REPORT, 0),
            RawInputEvent::new(EV_MSC, MSC_SCAN, 458760),
            RawInputEvent::new(EV_KEY, 18, 0),
            RawInputEvent::new(EV_SYN, SYN_REPORT, 0),
            RawInputEvent::new(EV_MSC, MSC_SCAN, 458774),
            RawInputEvent::new(EV_KEY, 31, 1),
            RawInputEvent::new(EV_SYN, SYN_REPORT, 0),
            RawInputEvent::new(EV_MSC, MSC_SCAN, 458774),
            RawInputEvent::new(EV_KEY, 31, 0),
            RawInputEvent::new(EV_SYN, SYN_REPORT, 0),
            RawInputEvent::new(EV_MSC, MSC_SCAN, 458775),
            RawInputEvent::new(EV_KEY, 20, 1),
            RawInputEvent::new(EV_SYN, SYN_REPORT, 0),
            RawInputEvent::new(EV_MSC, MSC_SCAN, 458775),
            RawInputEvent::new(EV_KEY, 20, 0),
            RawInputEvent::new(EV_SYN, SYN_REPORT, 0),
            RawInputEvent::new(EV_MSC, MSC_SCAN, 458976),
            RawInputEvent::new(EV_KEY, 29, 1),
            RawInputEvent::new(EV_SYN, SYN_REPORT, 0),
            RawInputEvent::new(EV_MSC, MSC_SCAN, 458758),
            RawInputEvent::new(EV_KEY, 46, 1),
            RawInputEvent::new(EV_SYN, SYN_REPORT, 0),
        ];

        let mut decoder = KeyboardEventDecoder::default();
        let decoded = decoder.decode_all(trace);

        let pressed = decoded
            .iter()
            .filter(|event| event.state == KeyState::Down)
            .map(|event| event.key_name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            pressed,
            vec!["KEY_T", "KEY_E", "KEY_S", "KEY_T", "KEY_LEFTCTRL", "KEY_C"]
        );

        assert_eq!(decoded[0].raw_scancode, Some(458775));
        assert_eq!(decoded[2].raw_scancode, Some(458760));
        assert_eq!(decoded[4].raw_scancode, Some(458774));
        assert_eq!(decoded[8].raw_scancode, Some(458976));
        assert_eq!(decoded[9].raw_scancode, Some(458758));

        assert!(decoded[8].modifiers.ctrl);
        assert!(decoded[9].modifiers.ctrl);
        assert!(!decoded[9].modifiers.shift);
        assert!(!decoded[9].modifiers.alt);
        assert!(!decoded[9].modifiers.meta);
    }

    #[test]
    fn ctrl_release_clears_modifier_state() {
        let trace = [
            RawInputEvent::new(EV_MSC, MSC_SCAN, 458976),
            RawInputEvent::new(EV_KEY, 29, 1),
            RawInputEvent::new(EV_SYN, SYN_REPORT, 0),
            RawInputEvent::new(EV_MSC, MSC_SCAN, 458976),
            RawInputEvent::new(EV_KEY, 29, 0),
            RawInputEvent::new(EV_SYN, SYN_REPORT, 0),
        ];

        let mut decoder = KeyboardEventDecoder::default();
        let decoded = decoder.decode_all(trace);

        assert!(decoded[0].modifiers.ctrl);
        assert!(!decoded[1].modifiers.ctrl);
    }
}