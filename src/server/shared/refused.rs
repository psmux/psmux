//! The requests the shared server does not run yet (#798 step 3), named the
//! way the user typed them, so an attached client's binding, prompt or menu
//! that asks for one says why nothing happened instead of doing nothing.
//!
//! A one shot command never gets here: the router refuses it with
//! `unknown command` before the protocol handler sees it. What arrives is a
//! request from an attached client's persistent connection.

use crate::types::CtrlReq;

/// The command a refused request stands for, or `None` for the requests the
/// server sends itself or a client sends without being asked (a finished
/// config job, cross session forwarding, control mode bookkeeping), which
/// would only flash a message the user did not cause.
pub(crate) fn command_name(req: &CtrlReq) -> Option<&'static str> {
    Some(match req {
        CtrlReq::BreakPaneReq { .. } => "break-pane",
        CtrlReq::JoinPane { .. } => "join-pane",
        CtrlReq::NewFloat { .. } => "new-pane",
        CtrlReq::RespawnWindow(..) => "respawn-window",
        CtrlReq::ResizeWindow(..) => "resize-window",
        CtrlReq::PipePane(..) => "pipe-pane",
        CtrlReq::MoveWindow { .. } => "move-window",
        CtrlReq::SwapWindow { .. } => "swap-window",
        CtrlReq::LinkWindowReq { .. } => "link-window",
        CtrlReq::UnlinkWindowReq { .. } => "unlink-window",
        CtrlReq::FindWindow(..) => "find-window",
        CtrlReq::RenameSession(..) => "rename-session",
        CtrlReq::SetSessionGroup(..) => "new-session -t",
        CtrlReq::ChooseBuffer(..) => "choose-buffer",
        CtrlReq::DisplayMenu(..) | CtrlReq::DisplayMenuDirect(..) => "display-menu",
        CtrlReq::MenuSelect(..) | CtrlReq::MenuNavigate(..) => "display-menu",
        CtrlReq::DisplayPopup(..) | CtrlReq::PopupInput(..) => "display-popup",
        CtrlReq::CustomizeMode
        | CtrlReq::CustomizeNavigate(..)
        | CtrlReq::CustomizeFilter(..)
        | CtrlReq::CustomizeEdit
        | CtrlReq::CustomizeEditCancel
        | CtrlReq::CustomizeEditConfirm
        | CtrlReq::CustomizeEditUpdate(..)
        | CtrlReq::CustomizeResetDefault => "customize-mode",
        CtrlReq::ListClients(..) | CtrlReq::ListClientsFormat(..) => "list-clients",
        CtrlReq::DetachAllClients(..) | CtrlReq::DetachAllOtherClients(..) => "detach-client -a",
        CtrlReq::ForceDetachClient(..) | CtrlReq::ForceDetachClientByTty(..) => "detach-client",
        CtrlReq::LockClient => "lock-client",
        CtrlReq::SuspendClient => "suspend-client",
        CtrlReq::RefreshClient => "refresh-client",
        CtrlReq::SetClientIgnoreSize(..) => "refresh-client -f",
        CtrlReq::SourceFile(..) => "source-file",
        CtrlReq::ConfigWarnings(..) => "show-messages",
        CtrlReq::ShowWindowOptions(..) => "show-window-options",
        CtrlReq::ListCommands(..) => "list-commands",
        CtrlReq::ShowPromptHistory(..) => "show-prompt-history",
        CtrlReq::ClearPromptHistory => "clear-prompt-history",
        CtrlReq::RunCommand(..) => "run-shell",
        CtrlReq::WaitFor(..) => "wait-for",
        // Not the user's doing: internal or bookkeeping requests.
        CtrlReq::ClaimSession(..)
        | CtrlReq::SourceJobDone { .. }
        | CtrlReq::SourceReplay(..)
        | CtrlReq::InFile(..)
        | CtrlReq::DebugStall(..)
        | CtrlReq::PaneForwardExtract(..)
        | CtrlReq::PaneForwardInject { .. }
        | CtrlReq::PaneForwardKill(..)
        | CtrlReq::PaneForwardResize(..)
        | CtrlReq::PaneForwardStatus(..)
        | CtrlReq::ControlRegister { .. }
        | CtrlReq::ControlDeregister { .. }
        | CtrlReq::ControlSubscribe { .. }
        | CtrlReq::ControlUnsubscribe { .. }
        | CtrlReq::ControlPausePane { .. }
        | CtrlReq::ControlContinuePane { .. }
        | CtrlReq::ControlSetPauseAfter { .. }
        | CtrlReq::ControlClientResize { .. } => return None,
        // A request with an arm in dispatch never reaches the refusal; a
        // variant added later lands here until it gets a name or an arm.
        _ => "this command",
    })
}

/// The status line text for a refused command.
pub(crate) fn message(name: &str) -> String {
    format!("not supported in shared server mode yet: {}", name)
}
