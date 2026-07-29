import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { TrayPopover } from "./tray-popover/TrayPopover";

const isPopover = window.location.hash === "#popover";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {isPopover ? <TrayPopover /> : <App />}
  </StrictMode>,
);
