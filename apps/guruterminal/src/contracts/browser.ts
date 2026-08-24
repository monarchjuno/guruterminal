export type BrowserBounds = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type BrowserTabState = {
  tab_id: string;
  url: string;
  title: string;
  loading: boolean;
};

export type BrowserTabEvent =
  | { type: "load_started"; tab_id: string; url: string }
  | { type: "load_finished"; tab_id: string; url: string }
  | { type: "title_changed"; tab_id: string; title: string }
  | { type: "open_requested"; tab_id: string; url: string }
  | {
      type: "navigation_blocked";
      tab_id: string;
      url: string;
      message: string;
    }
  | { type: "download_blocked"; tab_id: string; url: string };

export type BrowserHistoryDirection = "back" | "forward";

export type BrowserTabOpenRequest = {
  url: string;
  bounds: BrowserBounds;
  visible: boolean;
};

export type BrowserTabBoundsRequest = {
  tab_id: string;
  bounds: BrowserBounds;
  visible: boolean;
};
