//! 讀出 X11 畫面上每一扇看得見的頂層視窗的原始事實，交給 [`crate::own_windows`] 判斷。
//!
//! 頂層＝root 底下直接那一層。有視窗管理員的桌面上，那一層多半是管理員畫的
//! 外框，程式自己的視窗（掛著 `WM_STATE` 的那扇）包在框裡面；外框歸管理員的
//! 行程，所以「這扇是誰的」要連框裡面一起問：從頂層往下，問到程式視窗為止
//! （最多 [`FAMILY_DEPTH`] 層），其中任何一扇是她的，整個頂層就算她的。沒有框的
//! （選單、提示、沒被管理的視窗）也一樣往下問，只是問到的都是同一個程式。
//!
//! 「是誰的」問 X server（X-Resource 擴充）：它從連線認得出是哪個行程建的，
//! 程式自己改不了。程式自己掛的 `_NET_WM_PID` 不用——GTK 的彈出選單根本沒掛。
//!
//! 看不看得穿，讀得到的只有三種：32 位元色深（帶 alpha）、`_NET_WM_WINDOW_OPACITY`
//! 不是全不透明、形狀不是方的。框或框裡任何一扇有其中一種，就不拿它擋她。
//! 合成管理員自己的規則（例如「沒在用的視窗一律半透明」）從視窗上讀不到，
//! 照實心算。
//!
//! 這裡只送查詢，不改任何視窗、不建任何東西。

use std::collections::HashMap;

use anyhow::{Context, Result};
use x11rb::cookie::Cookie;
use x11rb::errors::ConnectionError;
use x11rb::protocol::res::{ClientIdMask, ClientIdSpec, ConnectionExt as _, QueryClientIdsReply};
use x11rb::protocol::shape::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, GetGeometryReply, GetPropertyReply, MapState, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::x11_utils::TryParse;

use super::{XAtoms, process_file_name};
use crate::own_windows::{DesktopRect, WindowFacts, is_her_program};

/// 從頂層往下最多問幾層。常見的視窗管理員是「外框→程式」，KWin 是
/// 「外框→包裝→程式」。
const FAMILY_DEPTH: usize = 3;

/// 每一扇看得見的頂層視窗，最上面的排第一個（`own_parts` 要的順序）。
///
/// 看不見的直接不收：沒 map 的（最小化、在別的工作區）、只收輸入畫不出東西的。
/// 中途任何一個查詢問不出來（例如一扇視窗剛好在這幾毫秒裡關掉），整批不要。
pub(super) fn window_facts(
    connection: &RustConnection,
    root: Window,
    atoms: &XAtoms,
) -> Result<Vec<WindowFacts>> {
    // QueryTree 由下往上排。
    let tops = connection
        .query_tree(root)?
        .reply()
        .context("列不出畫面上的視窗，這一拍沒有抓畫面")?
        .children;
    let attributes = all(tops
        .iter()
        .map(|&top| connection.get_window_attributes(top)))?;
    let shown: Vec<Window> = tops
        .iter()
        .zip(&attributes)
        .rev()
        .filter(|(_, attributes)| {
            attributes.map_state == MapState::VIEWABLE
                && attributes.class != WindowClass::INPUT_ONLY
        })
        .map(|(&top, _)| top)
        .collect();

    // 每一扇頂層要問的：它自己排第一個，再往下到程式視窗為止。
    let groups = family(connection, &shown, atoms.wm_state)?;
    let asked: Vec<Window> = groups.iter().flatten().copied().collect();
    let geometries = all(asked.iter().map(|&window| connection.get_geometry(window)))?;
    let opacities = all(asked.iter().map(|&window| {
        connection.get_property(
            false,
            window,
            atoms.window_opacity,
            AtomEnum::CARDINAL,
            0,
            1,
        )
    }))?;
    let shapes = all(asked
        .iter()
        .map(|&window| connection.shape_query_extents(window)))
    .context("X server 讀不出視窗的形狀，這一拍沒有抓畫面")?;
    let owners = all(asked.iter().map(|&window| {
        connection.res_query_client_ids(&[ClientIdSpec {
            client: window,
            mask: ClientIdMask::LOCAL_CLIENT_PID,
        }])
    }))
    .context("X server 認不出視窗是哪個程式建的，這一拍沒有抓畫面")?;

    // 同一個程式通常開好幾扇；pid 只在這一次列舉裡快取，下一拍重問——pid 會被回收。
    let mut names: HashMap<u32, Option<String>> = HashMap::new();
    let mut facts = Vec::with_capacity(groups.len());
    let mut start = 0;
    for group in &groups {
        let range = start..start + group.len();
        start = range.end;
        let exe_names: Vec<String> = owners[range.clone()]
            .iter()
            .filter_map(local_pid)
            .filter_map(|pid| {
                names
                    .entry(pid)
                    .or_insert_with(|| process_file_name(pid))
                    .clone()
            })
            .collect();
        let exe_name = exe_names
            .iter()
            .find(|name| is_her_program(Some(name.as_str())))
            .or(exe_names.first())
            .cloned();
        let see_through = geometries[range.clone()]
            .iter()
            .any(|geometry| geometry.depth == 32)
            || opacities[range.clone()].iter().any(partly_transparent);
        let shaped = shapes[range.clone()]
            .iter()
            .any(|shape| shape.bounding_shaped || shape.clip_shaped);
        // 頂層是 root 的子視窗，它的位置就是桌面座標。
        let rect = outer(&geometries[range.start]);
        facts.push(WindowFacts {
            exe_name,
            visible: true,
            // 縮到最小的視窗 X11 會 unmap，上面已經跳過。
            minimized: false,
            // X11 沒有「map 著卻被藏起來」這一格。有合成管理員的時候，畫在哪裡
            // 是它決定的，從這裡讀不到；照畫在自己的位置上算。
            cloaked: Some(false),
            see_through,
            shaped,
            window_rect: Some(rect),
            frame_bounds: Some(rect),
        });
    }
    Ok(facts)
}

/// 每一扇頂層和它底下的視窗，頂層排第一個。一路往下問到掛著 `WM_STATE` 的
/// 程式視窗為止（它自己收進來，不再往它裡面找），最多 [`FAMILY_DEPTH`] 層。
fn family(connection: &RustConnection, tops: &[Window], wm_state: u32) -> Result<Vec<Vec<Window>>> {
    let mut groups: Vec<Vec<Window>> = tops.iter().map(|&top| vec![top]).collect();
    let mut level: Vec<(usize, Window)> = tops.iter().copied().enumerate().collect();
    for _ in 0..FAMILY_DEPTH {
        let states = all(level.iter().map(|&(_, window)| {
            connection.get_property(false, window, wm_state, AtomEnum::ANY, 0, 0)
        }))?;
        let deeper: Vec<(usize, Window)> = level
            .iter()
            .zip(&states)
            .filter(|(_, state)| state.type_ == u32::from(AtomEnum::NONE))
            .map(|(&entry, _)| entry)
            .collect();
        let trees = all(deeper
            .iter()
            .map(|&(_, window)| connection.query_tree(window)))?;
        level = deeper
            .iter()
            .zip(trees)
            .flat_map(|(&(top, _), tree)| tree.children.into_iter().map(move |child| (top, child)))
            .collect();
        for &(top, window) in &level {
            groups[top].push(window);
        }
    }
    Ok(groups)
}

/// 先把一整批請求送出去再一起收，不要一扇一扇來回。任何一個問不出來，整批不要。
fn all<'c, R: TryParse>(
    sent: impl IntoIterator<Item = Result<Cookie<'c, RustConnection, R>, ConnectionError>>,
) -> Result<Vec<R>> {
    let cookies = sent.into_iter().collect::<Result<Vec<_>, _>>()?;
    cookies
        .into_iter()
        .map(|cookie| Ok(cookie.reply()?))
        .collect()
}

/// X server 從連線認出來的行程。遠端連進來的、行程已經結束而視窗還留著的，
/// 沒有。
fn local_pid(reply: &QueryClientIdsReply) -> Option<u32> {
    reply
        .ids
        .iter()
        .find(|id| id.spec.mask.contains(ClientIdMask::LOCAL_CLIENT_PID))
        .and_then(|id| id.value.first().copied())
}

/// `_NET_WM_WINDOW_OPACITY` 有掛、而且不是讀得出來的「完全不透明」。
fn partly_transparent(reply: &GetPropertyReply) -> bool {
    if reply.type_ == u32::from(AtomEnum::NONE) {
        return false;
    }
    !reply
        .value32()
        .and_then(|mut values| values.next())
        .is_some_and(|opacity| opacity == u32::MAX)
}

/// 外框（含邊框）在父視窗座標裡的範圍。
fn outer(geometry: &GetGeometryReply) -> DesktopRect {
    let border = 2 * i32::from(geometry.border_width);
    DesktopRect {
        left: i32::from(geometry.x),
        top: i32::from(geometry.y),
        right: i32::from(geometry.x) + i32::from(geometry.width) + border,
        bottom: i32::from(geometry.y) + i32::from(geometry.height) + border,
    }
}

#[cfg(test)]
mod tests {
    //! 真的開一台 Xvfb、真的開視窗，走產品那支 `capture_x11`。
    //!
    //! 「她」是改名成 `sister-desktop` 的 xmessage：別的行程開的一扇真的 X 視窗，
    //! X server 從連線認得出是誰開的，跟她自己的視窗走同一條路。別人的視窗由
    //! 測試自己開，X server 認得出那是測試程式。視窗管理員也由測試自己扮：開一個
    //! 框、把她搬進去、掛上 `WM_STATE`——跟真的管理員做的是同一件事。
    //!
    //! 期望的範圍一律由測試自己從 GetGeometry 算，不借產品的 `outer`。

    use super::*;
    use crate::linux::tests::Xvfb;
    use crate::linux::{capture_x11, foreground, grab_x11};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};
    use x11rb::connection::Connection;
    use x11rb::protocol::shape::{SK, SO};
    use x11rb::protocol::xproto::{
        ChangeWindowAttributesAux, ClipOrdering, ColormapAlloc, ConfigureWindowAux,
        CreateWindowAux, PropMode, Rectangle, StackMode,
    };
    use x11rb::wrapper::ConnectionExt as _;

    /// CI 的 X11 那一步設成 1：少了 Xvfb 或 xmessage 就要紅，不准安靜跳過。
    const REQUIRE: &str = "AI_SISTER_REQUIRE_X11_OWN_WINDOWS";
    const WIDTH: u16 = 240;
    const HEIGHT: u16 = 160;
    const GRAY: u32 = 0x80_80_80;
    const GREEN: u32 = 0x00_ff_00;
    const BLUE: u32 = 0x00_00_ff;
    const YELLOW: u32 = 0xff_ff_00;
    const BLACK: [u8; 4] = [0, 0, 0, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];

    fn required() -> bool {
        matches!(std::env::var(REQUIRE), Ok(value) if value == "1")
    }

    fn on_path(program: &str) -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|dir| dir.join(program))
            .find(|path| path.is_file())
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Area {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    impl Area {
        fn size(&self) -> usize {
            ((self.right - self.left) * (self.bottom - self.top)) as usize
        }
    }

    struct Shot {
        rgba: Vec<u8>,
    }

    impl Shot {
        fn at(&self, x: i32, y: i32) -> [u8; 4] {
            let at = (y as usize * usize::from(WIDTH) + x as usize) * 4;
            self.rgba[at..at + 4].try_into().expect("four bytes")
        }

        fn count(&self, area: Area, color: [u8; 4]) -> usize {
            (area.top..area.bottom)
                .flat_map(|y| (area.left..area.right).map(move |x| (x, y)))
                .filter(|&(x, y)| self.at(x, y) == color)
                .count()
        }

        fn count_all(&self, color: [u8; 4]) -> usize {
            self.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| **pixel == color)
                .count()
        }
    }

    fn rgb(pixel: u32) -> [u8; 4] {
        [(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8, 255]
    }

    /// 一台 Xvfb、一條測試自己的連線（扮別的程式和視窗管理員）、一條 recorder
    /// 的連線，和一個放「她」的程式檔的暫存資料夾。
    struct Desk {
        xvfb: Xvfb,
        own: RustConnection,
        recorder: RustConnection,
        screen: usize,
        root: Window,
        atoms: XAtoms,
        programs: PathBuf,
        xmessage: PathBuf,
        running: Vec<Child>,
    }

    impl Drop for Desk {
        fn drop(&mut self) {
            for child in &mut self.running {
                let _ = child.kill();
                let _ = child.wait();
            }
            let _ = std::fs::remove_dir_all(&self.programs);
        }
    }

    impl Desk {
        /// 少了 Xvfb 或 xmessage：平常跳過，[`REQUIRE`] 設了就紅。
        fn open() -> Option<Self> {
            let Some(xmessage) = on_path("xmessage") else {
                assert!(!required(), "{REQUIRE}=1 但找不到 xmessage");
                return None;
            };
            let geometry = format!("{WIDTH}x{HEIGHT}x24");
            let Some(xvfb) = Xvfb::start_with_geometry(&geometry) else {
                assert!(!required(), "{REQUIRE}=1 但找不到 Xvfb");
                return None;
            };
            let (own, screen) = RustConnection::connect(Some(&xvfb.display)).expect("連上 Xvfb");
            let (recorder, _) = RustConnection::connect(Some(&xvfb.display)).expect("連上 Xvfb");
            let root = own.setup().roots[screen].root;
            let atoms = XAtoms::new(&recorder).expect("atoms");
            let programs = std::env::temp_dir().join(format!(
                "sister-x11-own-windows-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            std::fs::create_dir_all(&programs).expect("暫存資料夾");
            let desk = Self {
                xvfb,
                own,
                recorder,
                screen,
                root,
                atoms,
                programs,
                xmessage,
                running: Vec::new(),
            };
            // 桌面底色換成灰的：Xvfb 預設的底紋有黑點，會跟塗黑的混在一起。
            desk.own
                .change_window_attributes(
                    root,
                    &ChangeWindowAttributesAux::new().background_pixel(GRAY),
                )
                .expect("root background")
                .check()
                .expect("root background reply");
            desk.own
                .clear_area(false, root, 0, 0, 0, 0)
                .expect("clear root")
                .check()
                .expect("clear root reply");
            Some(desk)
        }

        fn sync(&self) {
            self.own
                .get_input_focus()
                .expect("sync")
                .reply()
                .expect("sync reply");
        }

        fn shown_tops(&self) -> Vec<Window> {
            let tops = self
                .own
                .query_tree(self.root)
                .expect("query tree")
                .reply()
                .expect("query tree reply")
                .children;
            tops.into_iter()
                .filter(|&top| {
                    let attributes = self
                        .own
                        .get_window_attributes(top)
                        .expect("attributes")
                        .reply()
                        .expect("attributes reply");
                    attributes.map_state == MapState::VIEWABLE
                        && attributes.class == WindowClass::INPUT_OUTPUT
                })
                .collect()
        }

        /// 開一扇「她」的視窗：改名成 `sister-desktop` 的 xmessage，整片紅色
        /// （底、字、邊框都紅）。回傳它的行程和那扇頂層視窗。
        fn start_her(&mut self, geometry: &str) -> (u32, Window) {
            let program = self.programs.join("sister-desktop");
            if !program.exists() {
                std::fs::copy(&self.xmessage, &program).expect("把 xmessage 改名成她");
            }
            let before = self.shown_tops();
            let child = Command::new(&program)
                .env("DISPLAY", &self.xvfb.display)
                .args([
                    "-geometry",
                    geometry,
                    "-bg",
                    "red",
                    "-fg",
                    "red",
                    "-bd",
                    "red",
                    "-buttons",
                    "",
                    "AI SISTER",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("開她");
            let pid = child.id();
            self.running.push(child);
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let fresh: Vec<Window> = self
                    .shown_tops()
                    .into_iter()
                    .filter(|top| !before.contains(top))
                    .collect();
                if let [her] = fresh[..] {
                    return (pid, her);
                }
                assert!(
                    Instant::now() < deadline,
                    "她的視窗十秒內沒有出現，新出現的是 {fresh:?}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        /// 測試自己開的一扇視窗（X server 認得出是測試程式開的）。
        fn window(&self, parent: Window, area: Area, color: u32) -> Window {
            let screen = &self.own.setup().roots[self.screen];
            let window = self.own.generate_id().expect("window id");
            self.own
                .create_window(
                    screen.root_depth,
                    window,
                    parent,
                    area.left as i16,
                    area.top as i16,
                    (area.right - area.left) as u16,
                    (area.bottom - area.top) as u16,
                    0,
                    WindowClass::INPUT_OUTPUT,
                    screen.root_visual,
                    &CreateWindowAux::new().background_pixel(color),
                )
                .expect("create window")
                .check()
                .expect("create window reply");
            window
        }

        /// 32 位元色深（帶 alpha）的一扇。
        fn argb_window(&self, parent: Window, area: Area) -> Window {
            let screen = &self.own.setup().roots[self.screen];
            let visual = screen
                .allowed_depths
                .iter()
                .find(|depth| depth.depth == 32)
                .and_then(|depth| depth.visuals.first())
                .expect("Xvfb 有 32 位元色深的 visual")
                .visual_id;
            let colormap = self.own.generate_id().expect("colormap id");
            self.own
                .create_colormap(ColormapAlloc::NONE, colormap, self.root, visual)
                .expect("colormap")
                .check()
                .expect("colormap reply");
            let window = self.own.generate_id().expect("window id");
            self.own
                .create_window(
                    32,
                    window,
                    parent,
                    area.left as i16,
                    area.top as i16,
                    (area.right - area.left) as u16,
                    (area.bottom - area.top) as u16,
                    0,
                    WindowClass::INPUT_OUTPUT,
                    visual,
                    &CreateWindowAux::new()
                        .background_pixel(0x80_00_ff_00)
                        .border_pixel(0)
                        .colormap(colormap),
                )
                .expect("create argb window")
                .check()
                .expect("create argb window reply");
            window
        }

        fn map(&self, window: Window) {
            self.own
                .map_window(window)
                .expect("map")
                .check()
                .expect("map reply");
            self.sync();
        }

        fn opacity(&self, window: Window, opacity: u32) {
            self.own
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    self.atoms.window_opacity,
                    AtomEnum::CARDINAL,
                    &[opacity],
                )
                .expect("opacity")
                .check()
                .expect("opacity reply");
        }

        /// 視窗管理員對程式視窗做的那件事：`WM_STATE` = NormalState。
        fn managed(&self, client: Window) {
            self.own
                .change_property32(
                    PropMode::REPLACE,
                    client,
                    self.atoms.wm_state,
                    self.atoms.wm_state,
                    &[1, 0],
                )
                .expect("WM_STATE")
                .check()
                .expect("WM_STATE reply");
        }

        fn reparent(&self, window: Window, parent: Window, x: i16, y: i16) {
            self.own
                .reparent_window(window, parent, x, y)
                .expect("reparent")
                .check()
                .expect("reparent reply");
        }

        fn destroy(&self, window: Window) {
            self.own
                .destroy_window(window)
                .expect("destroy")
                .check()
                .expect("destroy reply");
            self.sync();
        }

        /// 外框（含邊框）的範圍，測試自己從 GetGeometry 算。
        fn outer(&self, window: Window) -> Area {
            let geometry = self
                .own
                .get_geometry(window)
                .expect("geometry")
                .reply()
                .expect("geometry reply");
            let border = 2 * i32::from(geometry.border_width);
            Area {
                left: i32::from(geometry.x),
                top: i32::from(geometry.y),
                right: i32::from(geometry.x) + i32::from(geometry.width) + border,
                bottom: i32::from(geometry.y) + i32::from(geometry.height) + border,
            }
        }

        /// 沒塗過的原始畫面。
        fn raw(&self) -> Shot {
            Shot {
                rgba: grab_x11(&self.recorder, self.screen, WIDTH, HEIGHT).expect("raw grab"),
            }
        }

        /// 產品那一支：抓、塗，交給 dhash／OCR 的那一張。
        fn capture(&self) -> Shot {
            let frame = capture_x11(&self.recorder, self.screen, &self.atoms, 7).expect("capture");
            assert_eq!(
                (frame.width, frame.height),
                (u32::from(WIDTH), u32::from(HEIGHT))
            );
            Shot {
                rgba: frame.rgba.expect("frame pixels"),
            }
        }

        /// 等到原始畫面上 `area` 幾乎整塊都是 `color`——xmessage 畫好了才開始量。
        fn wait_painted(&self, area: Area, color: [u8; 4]) {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let painted = self.raw().count(area, color);
                if painted * 10 >= area.size() * 9 {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "{area:?} 十秒內只有 {painted}/{} 個 {color:?}",
                    area.size()
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    #[test]
    fn her_window_is_black_in_her_capture_and_nothing_else_is() {
        let Some(mut desk) = Desk::open() else { return };
        let other = desk.window(
            desk.root,
            Area {
                left: 170,
                top: 10,
                right: 220,
                bottom: 60,
            },
            GREEN,
        );
        desk.map(other);
        let (_, her) = desk.start_her("100x60+10+10");
        let hers = desk.outer(her);
        desk.wait_painted(hers, RED);
        let raw = desk.raw();
        assert_eq!(raw.count_all(BLACK), 0, "還沒塗之前畫面上不該有黑的");

        let facts = window_facts(&desk.recorder, desk.root, &desk.atoms).expect("facts");
        let names: Vec<Option<&str>> = facts.iter().map(|f| f.exe_name.as_deref()).collect();
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == Some("sister-desktop"))
                .count(),
            1,
            "X server 要認得出那扇是她：{names:?}"
        );

        let shot = desk.capture();
        assert_eq!(
            shot.count(hers, BLACK),
            hers.size(),
            "她那一塊（含邊框）要全黑"
        );
        assert_eq!(shot.count_all(BLACK), hers.size(), "黑的只能是她那一塊");
        let others = desk.outer(other);
        assert_eq!(
            shot.count(others, rgb(GREEN)),
            others.size(),
            "別人的視窗原封不動"
        );
        assert_eq!(shot.at(150, 140), rgb(GRAY), "桌面原封不動");
    }

    #[test]
    fn an_opaque_window_above_her_keeps_the_part_it_covers() {
        let Some(mut desk) = Desk::open() else { return };
        let (_, her) = desk.start_her("100x60+10+10");
        let hers = desk.outer(her);
        desk.wait_painted(hers, RED);
        // 後開的在上面。
        let cover = Area {
            left: 60,
            top: 30,
            right: 160,
            bottom: 110,
        };
        let above = desk.window(desk.root, cover, BLUE);
        desk.map(above);

        let shot = desk.capture();
        let overlap = Area {
            left: cover.left,
            top: cover.top,
            right: hers.right,
            bottom: hers.bottom,
        };
        assert_eq!(
            shot.count(cover, rgb(BLUE)),
            cover.size(),
            "蓋在她上面的那扇整塊留著"
        );
        assert_eq!(
            shot.count_all(BLACK),
            hers.size() - overlap.size(),
            "她露出來的部分全黑，被蓋住的不塗"
        );
        assert_eq!(shot.at(hers.left, hers.top), BLACK);
        assert_eq!(shot.at(hers.right - 1, cover.top - 1), BLACK);
    }

    #[test]
    fn a_window_she_is_completely_behind_leaves_nothing_black() {
        let Some(mut desk) = Desk::open() else { return };
        let (_, her) = desk.start_her("100x60+10+10");
        let hers = desk.outer(her);
        desk.wait_painted(hers, RED);
        let cover = Area {
            left: 0,
            top: 0,
            right: 200,
            bottom: 120,
        };
        let above = desk.window(desk.root, cover, BLUE);
        desk.map(above);

        let shot = desk.capture();
        assert_eq!(shot.count_all(BLACK), 0, "她整扇被實心的視窗蓋住，不必塗");
        assert_eq!(shot.count(cover, rgb(BLUE)), cover.size());
    }

    /// 蓋在她上面、可能看得穿的，都不算擋住。同一個迴圈裡有一臂是真的實心，
    /// 那一臂要**不**塗——證明量得出兩種結果。
    #[test]
    fn windows_she_may_show_through_do_not_hide_her() {
        let Some(mut desk) = Desk::open() else { return };
        let (_, her) = desk.start_her("100x60+10+10");
        let hers = desk.outer(her);
        desk.wait_painted(hers, RED);
        let cover = Area {
            left: 0,
            top: 0,
            right: 200,
            bottom: 120,
        };
        let frame_area = cover;
        let client_area = Area {
            left: 0,
            top: 20,
            right: 200,
            bottom: 120,
        };

        type Build = fn(&Desk, Area, Area) -> Window;
        let arms: [(&str, bool, Build); 9] = [
            (
                "實心：不透明度掛滿 0xFFFFFFFF",
                false,
                |desk, cover, _| {
                    let window = desk.window(desk.root, cover, BLUE);
                    desk.opacity(window, u32::MAX);
                    window
                },
            ),
            ("32 位元色深", true, |desk, cover, _| {
                desk.argb_window(desk.root, cover)
            }),
            (
                "_NET_WM_WINDOW_OPACITY 半透明",
                true,
                |desk, cover, _| {
                    let window = desk.window(desk.root, cover, BLUE);
                    desk.opacity(window, 0x8000_0000);
                    window
                },
            ),
            (
                "_NET_WM_WINDOW_OPACITY 讀不懂",
                true,
                |desk, cover, _| {
                    let window = desk.window(desk.root, cover, BLUE);
                    desk.own
                        .change_property8(
                            PropMode::REPLACE,
                            window,
                            desk.atoms.window_opacity,
                            AtomEnum::STRING,
                            b"opaque",
                        )
                        .expect("opacity text")
                        .check()
                        .expect("opacity text reply");
                    window
                },
            ),
            ("形狀不是方的", true, |desk, cover, _| {
                let window = desk.window(desk.root, cover, BLUE);
                desk.own
                    .shape_rectangles(
                        SO::SET,
                        SK::BOUNDING,
                        ClipOrdering::UNSORTED,
                        window,
                        0,
                        0,
                        &[Rectangle {
                            x: 0,
                            y: 0,
                            width: 150,
                            height: 120,
                        }],
                    )
                    .expect("shape")
                    .check()
                    .expect("shape reply");
                window
            }),
            ("clip 形狀", true, |desk, cover, _| {
                let window = desk.window(desk.root, cover, BLUE);
                desk.own
                    .shape_rectangles(
                        SO::SET,
                        SK::CLIP,
                        ClipOrdering::UNSORTED,
                        window,
                        0,
                        0,
                        &[Rectangle {
                            x: 0,
                            y: 0,
                            width: 150,
                            height: 120,
                        }],
                    )
                    .expect("clip shape")
                    .check()
                    .expect("clip shape reply");
                window
            }),
            (
                "框是實心的、框裡的程式視窗半透明",
                true,
                |desk, frame, client| {
                    let top = desk.window(desk.root, frame, YELLOW);
                    let inside = desk.window(top, client, BLUE);
                    desk.managed(inside);
                    desk.opacity(inside, 0x8000_0000);
                    desk.map(inside);
                    top
                },
            ),
            (
                "框是實心的、框裡的程式視窗 32 位元",
                true,
                |desk, frame, client| {
                    let top = desk.window(desk.root, frame, YELLOW);
                    let inside = desk.argb_window(top, client);
                    desk.managed(inside);
                    desk.map(inside);
                    top
                },
            ),
            (
                "框是實心的、框裡的程式視窗形狀不是方的",
                true,
                |desk, frame, client| {
                    let top = desk.window(desk.root, frame, YELLOW);
                    let inside = desk.window(top, client, BLUE);
                    desk.managed(inside);
                    desk.own
                        .shape_rectangles(
                            SO::SET,
                            SK::BOUNDING,
                            ClipOrdering::UNSORTED,
                            inside,
                            0,
                            0,
                            &[Rectangle {
                                x: 0,
                                y: 0,
                                width: 150,
                                height: 100,
                            }],
                        )
                        .expect("client shape")
                        .check()
                        .expect("client shape reply");
                    desk.map(inside);
                    top
                },
            ),
        ];
        let mut wrong = Vec::new();
        for (name, see_through, build) in arms {
            let window = build(&desk, frame_area, client_area);
            desk.map(window);
            let black = desk.capture().count(hers, BLACK);
            let want = if see_through { hers.size() } else { 0 };
            if black != want {
                wrong.push(format!("{name}：她那一塊黑了 {black}，應該是 {want}"));
            }
            desk.destroy(window);
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    #[test]
    fn windows_that_draw_nothing_do_not_hide_her() {
        let Some(mut desk) = Desk::open() else { return };
        let (_, her) = desk.start_her("100x60+10+10");
        let hers = desk.outer(her);
        desk.wait_painted(hers, RED);
        let cover = Area {
            left: 0,
            top: 0,
            right: 200,
            bottom: 120,
        };

        let screen = &desk.own.setup().roots[desk.screen];
        let input_only = desk.own.generate_id().expect("window id");
        desk.own
            .create_window(
                0,
                input_only,
                desk.root,
                0,
                0,
                200,
                120,
                0,
                WindowClass::INPUT_ONLY,
                screen.root_visual,
                &CreateWindowAux::new(),
            )
            .expect("input only")
            .check()
            .expect("input only reply");
        desk.map(input_only);
        assert_eq!(
            desk.capture().count(hers, BLACK),
            hers.size(),
            "只收輸入的視窗畫不出東西，擋不住她"
        );
        desk.destroy(input_only);

        let unmapped = desk.window(desk.root, cover, BLUE);
        desk.sync();
        assert_eq!(
            desk.capture().count(hers, BLACK),
            hers.size(),
            "沒 map 的視窗擋不住她"
        );
        // 同一扇 map 上去就擋住了：證明上面那句量得出差別。
        desk.map(unmapped);
        assert_eq!(desk.capture().count(hers, BLACK), 0);
    }

    #[test]
    fn she_is_not_blacked_out_where_she_is_not_shown() {
        let Some(mut desk) = Desk::open() else { return };
        let (_, her) = desk.start_her("100x60+10+10");
        let hers = desk.outer(her);
        desk.wait_painted(hers, RED);
        assert_eq!(desk.capture().count_all(BLACK), hers.size());
        desk.own
            .unmap_window(her)
            .expect("unmap")
            .check()
            .expect("unmap reply");
        desk.sync();
        assert_eq!(
            desk.capture().count_all(BLACK),
            0,
            "她縮起來了，畫面上沒有她就不塗"
        );
    }

    /// 視窗管理員把她包進自己的框裡：框是管理員的行程開的，框上的標題列也
    /// 一起塗掉。三種包法：一層框、框裡再一層包裝（KWin）、框裡的她還沒掛
    /// `WM_STATE`。
    #[test]
    fn a_frame_she_is_inside_is_all_hers() {
        for (name, wrapper, state) in [
            ("框→她", false, true),
            ("框→包裝→她", true, true),
            ("框→她（還沒掛 WM_STATE）", false, false),
        ] {
            let Some(mut desk) = Desk::open() else { return };
            let (_, her) = desk.start_her("100x60+10+10");
            let frame = desk.window(
                desk.root,
                Area {
                    left: 30,
                    top: 20,
                    right: 160,
                    bottom: 110,
                },
                YELLOW,
            );
            let parent = if wrapper {
                let inside = desk.window(
                    frame,
                    Area {
                        left: 0,
                        top: 18,
                        right: 130,
                        bottom: 90,
                    },
                    YELLOW,
                );
                desk.map(inside);
                inside
            } else {
                frame
            };
            desk.reparent(her, parent, 4, if wrapper { 4 } else { 22 });
            if state {
                desk.managed(her);
            }
            desk.map(frame);
            let framed = desk.outer(frame);
            let title = Area {
                left: framed.left,
                top: framed.top,
                right: framed.right,
                bottom: framed.top + 18,
            };
            desk.wait_painted(title, rgb(YELLOW));
            let raw = desk.raw();
            assert!(
                raw.count(framed, RED) > 0,
                "{name}：她要真的畫在框裡，不然這條測的是空框"
            );

            let shot = desk.capture();
            assert_eq!(
                shot.count(framed, BLACK),
                framed.size(),
                "{name}：整個框（含標題列）都要黑"
            );
            assert_eq!(shot.count_all(BLACK), framed.size(), "{name}：框外不塗");
        }
    }

    /// 她自己的視窗上掛著別人的 `_NET_WM_PID`，照樣是她：認人問的是 X server。
    #[test]
    fn the_x_server_says_whose_window_it_is_not_the_window() {
        let Some(mut desk) = Desk::open() else { return };
        let (_, her) = desk.start_her("100x60+10+10");
        let hers = desk.outer(her);
        desk.wait_painted(hers, RED);
        let pid_atom = desk
            .own
            .intern_atom(false, b"_NET_WM_PID")
            .expect("atom")
            .reply()
            .expect("atom reply")
            .atom;
        desk.own
            .change_property32(
                PropMode::REPLACE,
                her,
                pid_atom,
                AtomEnum::CARDINAL,
                &[std::process::id()],
            )
            .expect("pid")
            .check()
            .expect("pid reply");
        desk.sync();
        assert_eq!(desk.capture().count(hers, BLACK), hers.size());
    }

    /// 套件升級會在她還開著的時候換掉程式檔。之後 `/proc/<pid>/exe` 多一段
    /// ` (deleted)`；那還是她——塗黑和前景那道閘門都要認得。
    #[test]
    fn she_is_still_her_after_her_program_file_is_replaced() {
        let Some(mut desk) = Desk::open() else { return };
        let (pid, her) = desk.start_her("100x60+10+10");
        let hers = desk.outer(her);
        desk.wait_painted(hers, RED);
        std::fs::remove_file(desk.programs.join("sister-desktop")).expect("換掉她的程式檔");
        let link = std::fs::read_link(format!("/proc/{pid}/exe")).expect("exe link");
        assert!(
            link.to_string_lossy().ends_with(" (deleted)"),
            "這條要測的是被換掉的程式檔，實際是 {link:?}"
        );

        assert_eq!(desk.capture().count(hers, BLACK), hers.size(), "還是要塗");

        // 前景那道閘門：她在前景時整拍不錄，認的是同一個檔名。
        let atoms = &desk.atoms;
        desk.own
            .change_property32(
                PropMode::REPLACE,
                her,
                atoms.pid,
                AtomEnum::CARDINAL,
                &[pid],
            )
            .expect("pid")
            .check()
            .expect("pid reply");
        desk.own
            .change_property32(
                PropMode::REPLACE,
                desk.root,
                atoms.active_window,
                AtomEnum::WINDOW,
                &[her],
            )
            .expect("active")
            .check()
            .expect("active reply");
        desk.sync();
        let identity = foreground(&desk.recorder, desk.screen, atoms).expect("foreground");
        assert_eq!(identity.app_id.as_deref(), Some("sister-desktop"));
    }

    /// 她被拉到別人上面也照樣塗；別人那扇露出來的部分不動。
    #[test]
    fn she_is_black_when_raised_above_another_window() {
        let Some(mut desk) = Desk::open() else { return };
        let below = Area {
            left: 60,
            top: 30,
            right: 160,
            bottom: 110,
        };
        let other = desk.window(desk.root, below, BLUE);
        desk.map(other);
        let (_, her) = desk.start_her("100x60+10+10");
        let hers = desk.outer(her);
        desk.wait_painted(hers, RED);
        desk.own
            .configure_window(her, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))
            .expect("raise")
            .check()
            .expect("raise reply");
        desk.sync();

        let shot = desk.capture();
        assert_eq!(shot.count(hers, BLACK), hers.size());
        assert_eq!(shot.count_all(BLACK), hers.size());
        let overlap = Area {
            left: below.left,
            top: below.top,
            right: hers.right,
            bottom: hers.bottom,
        };
        assert_eq!(shot.count(below, rgb(BLUE)), below.size() - overlap.size());
    }
}
