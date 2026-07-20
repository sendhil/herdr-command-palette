use std::io;

use crossterm::{
    cursor::{Hide, Show},
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

pub trait TerminalOps {
    fn enable_raw_mode(&mut self) -> io::Result<()>;
    fn enter_alternate_screen(&mut self) -> io::Result<()>;
    fn enable_mouse_capture(&mut self) -> io::Result<()>;
    fn hide_cursor(&mut self) -> io::Result<()>;
    fn show_cursor(&mut self) -> io::Result<()>;
    fn disable_mouse_capture(&mut self) -> io::Result<()>;
    fn leave_alternate_screen(&mut self) -> io::Result<()>;
    fn disable_raw_mode(&mut self) -> io::Result<()>;
}

pub struct TerminalGuard<O: TerminalOps> {
    ops: O,
    raw_mode: bool,
    alternate_screen: bool,
    mouse_capture: bool,
    cursor_hidden: bool,
}

impl<O: TerminalOps> TerminalGuard<O> {
    pub fn enter(ops: O) -> io::Result<Self> {
        let mut guard = Self {
            ops,
            raw_mode: false,
            alternate_screen: false,
            mouse_capture: false,
            cursor_hidden: false,
        };
        guard.ops.enable_raw_mode()?;
        guard.raw_mode = true;
        guard.ops.enter_alternate_screen()?;
        guard.alternate_screen = true;
        guard.ops.enable_mouse_capture()?;
        guard.mouse_capture = true;
        guard.ops.hide_cursor()?;
        guard.cursor_hidden = true;
        Ok(guard)
    }
}

impl<O: TerminalOps> Drop for TerminalGuard<O> {
    fn drop(&mut self) {
        if self.cursor_hidden {
            let _ = self.ops.show_cursor();
        }
        if self.mouse_capture {
            let _ = self.ops.disable_mouse_capture();
        }
        if self.alternate_screen {
            let _ = self.ops.leave_alternate_screen();
        }
        if self.raw_mode {
            let _ = self.ops.disable_raw_mode();
        }
    }
}

#[derive(Default)]
pub struct CrosstermTerminalOps;

impl TerminalOps for CrosstermTerminalOps {
    fn enable_raw_mode(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnterAlternateScreen)
    }

    fn enable_mouse_capture(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnableMouseCapture)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(io::stdout(), Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(io::stdout(), Show)
    }

    fn disable_mouse_capture(&mut self) -> io::Result<()> {
        execute!(io::stdout(), DisableMouseCapture)
    }

    fn leave_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), LeaveAlternateScreen)
    }

    fn disable_raw_mode(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io;
    use std::rc::Rc;

    use super::{TerminalGuard, TerminalOps};

    #[derive(Clone, Default)]
    struct Recorder(Rc<RefCell<Vec<&'static str>>>);

    impl TerminalOps for Recorder {
        fn enable_raw_mode(&mut self) -> io::Result<()> {
            self.0.borrow_mut().push("raw-on");
            Ok(())
        }
        fn enter_alternate_screen(&mut self) -> io::Result<()> {
            self.0.borrow_mut().push("alt-on");
            Ok(())
        }
        fn enable_mouse_capture(&mut self) -> io::Result<()> {
            self.0.borrow_mut().push("mouse-on");
            Ok(())
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            self.0.borrow_mut().push("cursor-hide");
            Ok(())
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            self.0.borrow_mut().push("cursor-show");
            Ok(())
        }
        fn disable_mouse_capture(&mut self) -> io::Result<()> {
            self.0.borrow_mut().push("mouse-off");
            Ok(())
        }
        fn leave_alternate_screen(&mut self) -> io::Result<()> {
            self.0.borrow_mut().push("alt-off");
            Ok(())
        }
        fn disable_raw_mode(&mut self) -> io::Result<()> {
            self.0.borrow_mut().push("raw-off");
            Ok(())
        }
    }

    #[derive(Clone)]
    struct FailingRecorder {
        recorder: Recorder,
        fail_on: &'static str,
    }

    impl TerminalOps for FailingRecorder {
        fn enable_raw_mode(&mut self) -> io::Result<()> {
            self.recorder.enable_raw_mode()
        }
        fn enter_alternate_screen(&mut self) -> io::Result<()> {
            if self.fail_on == "alt-on" {
                Err(io::Error::other("alternate screen failed"))
            } else {
                self.recorder.enter_alternate_screen()
            }
        }
        fn enable_mouse_capture(&mut self) -> io::Result<()> {
            if self.fail_on == "mouse-on" {
                Err(io::Error::other("mouse failed"))
            } else {
                self.recorder.enable_mouse_capture()
            }
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            if self.fail_on == "cursor-hide" {
                Err(io::Error::other("cursor failed"))
            } else {
                self.recorder.hide_cursor()
            }
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            self.recorder.show_cursor()
        }
        fn disable_mouse_capture(&mut self) -> io::Result<()> {
            self.recorder.disable_mouse_capture()
        }
        fn leave_alternate_screen(&mut self) -> io::Result<()> {
            self.recorder.leave_alternate_screen()
        }
        fn disable_raw_mode(&mut self) -> io::Result<()> {
            self.recorder.disable_raw_mode()
        }
    }

    #[test]
    fn partial_setup_restores_only_successfully_enabled_stages_in_reverse_order() {
        for (failed_stage, expected) in [
            ("alt-on", vec!["raw-on", "raw-off"]),
            ("mouse-on", vec!["raw-on", "alt-on", "alt-off", "raw-off"]),
            (
                "cursor-hide",
                vec![
                    "raw-on",
                    "alt-on",
                    "mouse-on",
                    "mouse-off",
                    "alt-off",
                    "raw-off",
                ],
            ),
        ] {
            let recorder = Recorder::default();
            let events = recorder.0.clone();
            assert!(TerminalGuard::enter(FailingRecorder {
                recorder,
                fail_on: failed_stage
            })
            .is_err());
            assert_eq!(
                events.borrow().as_slice(),
                expected.as_slice(),
                "{failed_stage}"
            );
        }
    }

    #[test]
    fn restores_all_successful_stages_in_reverse_order_on_return_error_and_panic() {
        for path in ["return", "error", "escape", "panic"] {
            let recorder = Recorder::default();
            let events = recorder.0.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match path {
                "return" | "escape" => {
                    let _guard = TerminalGuard::enter(recorder).unwrap();
                }
                "error" => {
                    let result: Result<(), ()> = {
                        let _guard = TerminalGuard::enter(recorder).unwrap();
                        Err(())
                    };
                    assert!(result.is_err());
                }
                "panic" => {
                    let _guard = TerminalGuard::enter(recorder).unwrap();
                    panic!("test panic");
                }
                _ => unreachable!(),
            }));
            assert_eq!(result.is_err(), path == "panic");
            assert_eq!(
                &*events.borrow(),
                &[
                    "raw-on",
                    "alt-on",
                    "mouse-on",
                    "cursor-hide",
                    "cursor-show",
                    "mouse-off",
                    "alt-off",
                    "raw-off"
                ]
            );
        }
    }
}
