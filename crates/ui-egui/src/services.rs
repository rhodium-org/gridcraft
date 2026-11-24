use super::{host_places, printing};

type PickSave = Box<dyn Fn(&str) -> Option<String>>;
type OpenUrl = Box<dyn Fn(&str)>;
type CopyHtml = Box<dyn FnMut(&str, &str) -> Result<(), String>>;
type Download = Box<dyn Fn(&str, &[u8])>;
type PrintPdf = Box<dyn Fn(&[u8])>;
type HostOpen = Box<dyn Fn(&str, Option<&str>) -> Result<(), String>>;

/// Host services the platform shell provides (file pickers, opening URLs).
#[derive(Default)]
pub struct Services {
    /// Desktop: discover configured printers on a worker.
    pub printers: Option<printing::Discover>,
    /// Desktop: submit the PDF with the chosen settings on a worker.
    pub print_native: Option<printing::Submit>,
    pub pick_open: Option<Box<dyn Fn() -> Option<String>>>,
    pub pick_save: Option<PickSave>,
    /// Native picture picker (PNG/JPEG), separate from workbook opening.
    pub pick_picture: Option<Box<dyn Fn() -> Option<String>>>,
    /// Browser picture picker. Each dialog owns its inbox, so canceled picks cannot
    /// insert into a subsequent dialog or open the image as a workbook.
    pub pick_picture_async: Option<Box<dyn Fn(Inbox)>>,
    pub open_url: Option<OpenUrl>,
    /// Publish HTML and its plain-text alternative together. `Ok` means the host owns the
    /// write (including any asynchronous fallback); an immediate error uses egui's text path.
    pub copy_html: Option<CopyHtml>,
    /// Read the HTML flavour of the system clipboard when the user pastes text, so cells the
    /// source spreadsheet recorded as numbers stay numbers ("Rp10,000.00" → 10000). `None` from
    /// the host pastes the plain text alone.
    pub paste_html: Option<Box<dyn FnMut() -> Option<String>>>,
    /// Read the system clipboard's plain text for the ribbon and menu Paste commands (Cmd/Ctrl+V
    /// gets it from egui's `Event::Paste`). `None` from the host, or no service (web), pastes
    /// GridCraft's own clipboard.
    pub paste_text: Option<Box<dyn FnMut() -> Option<String>>>,
    /// Read the picture on the system clipboard for the ribbon and menu Paste commands when it
    /// holds no text (Cmd/Ctrl+V gets it from egui's `Event::PasteImage`).
    pub paste_image: Option<Box<dyn FnMut() -> Option<image::RgbaImage>>>,
    /// Web: deliver a file to the user (name, bytes). Hosted, the page gets it instead: exports
    /// (PDF, CSV, web page), never waited for.
    pub download: Option<Download>,
    /// Web, hosted: hand a workbook Save or Save As (.xlsx) to the host page
    /// ([`host_places`]), instead of downloading it. An error means the page didn't get it, and
    /// the save failed.
    pub host_save: Option<HostSaveFn>,
    /// Web, hosted: the places the host page keeps workbooks in and its switches (AutoSave,
    /// confirmed saves), written by the bridge from the host's messages ([`host_places`]).
    pub host_places: Option<host_places::HostPlaces>,
    /// Web, hosted: ask the host to open a workbook from one of its places
    /// (`gridcraft:open-request`): `(place, key)`, the key naming a workbook the host said lived
    /// there (a Recent entry), or none for the host to ask which. The workbook comes back as any
    /// host file does, through `inbox`. An error means the host wasn't asked.
    pub host_open: Option<HostOpen>,
    /// Web: start an asynchronous file pick; the bytes arrive later in `inbox`.
    pub open_async: Option<Box<dyn Fn()>>,
    /// Files delivered asynchronously (name, bytes), opened on the next frame.
    pub inbox: Option<Inbox>,
    /// Web: hand PDF bytes to the browser's print flow directly (no download,
    /// no intermediate file) — opens the system print dialog on that PDF.
    pub print: Option<PrintPdf>,
}

/// [`Services::host_save`]: `(file name, bytes, what kind of save)`.
pub type HostSaveFn = Box<dyn Fn(&str, &[u8], &host_places::HostSave) -> Result<(), String>>;

/// Shared queue of files read asynchronously (browser file picker, drag and drop).
pub type Inbox = std::sync::Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

/// Which parts of the app's own window chrome are drawn. Everything is on by default; a host
/// that embeds the sheet in its own window (and owns files itself) can turn off the title bar
/// and the File tab, whose Backstage view then never opens. Not persisted: it describes the
/// host, not a user preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chrome {
    /// The title bar: AutoSave, Home, Save, Undo/Redo, the file name, Comments, Share, Search and
    /// the theme switch.
    pub title_bar: bool,
    /// The ribbon's File tab and the Backstage view it opens (also reached by keyboard and
    /// commands such as `ui.backstage`).
    pub file_tab: bool,
}

impl Default for Chrome {
    fn default() -> Self {
        Chrome { title_bar: true, file_tab: true }
    }
}
