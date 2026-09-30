import {
  DynamicBorder,
  type ExtensionContext,
  type KeybindingsManager,
  type Theme,
} from "@earendil-works/pi-coding-agent";
import {
  Container,
  type SelectItem,
  SelectList,
  Spacer,
  Text,
  type TUI,
} from "@earendil-works/pi-tui";
import {
  formatAccountStatus,
  type QuotaAccount,
} from "./picker.ts";
import { reauthAccount } from "./usage.ts";

const AUTO_VALUE = "auto";

export async function selectCodexAccount(
  ctx: ExtensionContext,
  accounts: readonly QuotaAccount[],
  current?: string,
  mode: "auto" | "pinned" = "auto",
): Promise<string | undefined> {
  const items: SelectItem[] = [
    {
      value: AUTO_VALUE,
      label: "auto",
      description:
        mode === "auto"
          ? "highest remaining  ·  current"
          : "highest remaining",
    },
    ...accounts.map((account) => {
      const currentMark =
        account.name === current && mode === "pinned" ? "  ·  current" : "";
      return {
        value: account.name,
        label: account.name,
        description: `${formatAccountStatus(account)}${currentMark}`,
      };
    }),
  ];

  return ctx.ui.custom<string | undefined>((tui, theme, _keybindings, done) => {
    const container = new Container();
    container.addChild(new DynamicBorder((text) => theme.fg("accent", text)));
    container.addChild(
      new Text(theme.fg("accent", theme.bold("Codex account")), 1, 0),
    );
    const selectList = new SelectList(items, Math.min(items.length, 12), {
      selectedPrefix: (text) => theme.fg("accent", text),
      selectedText: (text) => theme.fg("accent", text),
      description: (text) => theme.fg("muted", text),
      scrollInfo: (text) => theme.fg("dim", text),
      noMatch: (text) => theme.fg("warning", text),
    });
    selectList.onSelect = (item) => done(item.value);
    selectList.onCancel = () => done(undefined);
    container.addChild(selectList);
    container.addChild(
      new Text(
        theme.fg("dim", "↑↓ navigate · enter select · esc cancel"),
        1,
        0,
      ),
    );
    container.addChild(new DynamicBorder((text) => theme.fg("accent", text)));
    return {
      render: (width) => container.render(width),
      invalidate: () => container.invalidate(),
      handleInput: (data) => {
        selectList.handleInput(data);
        tui.requestRender();
      },
    };
  });
}

export async function reauthWithDialog(
  ctx: ExtensionContext,
  name: string,
): Promise<boolean> {
  return ctx.ui.custom<boolean>((tui, theme, keybindings, done) => {
    const dialog = new ReauthDialog(tui, theme, keybindings, name, done);
    void dialog.start();
    return dialog;
  });
}

class ReauthDialog {
  private readonly container = new Container();
  private readonly body = new Container();
  private finished = false;

  constructor(
    private readonly tui: TUI,
    private readonly theme: Theme,
    private readonly keybindings: KeybindingsManager,
    private readonly name: string,
    private readonly done: (result: boolean) => void,
  ) {
    this.container.addChild(
      new DynamicBorder((text) => this.theme.fg("accent", text)),
    );
    this.container.addChild(
      new Text(
        this.theme.fg("accent", this.theme.bold(`Reauth Codex  ${this.name}`)),
        1,
        0,
      ),
    );
    this.container.addChild(this.body);
    this.container.addChild(
      new DynamicBorder((text) => this.theme.fg("accent", text)),
    );
    this.showMessage("Starting device login...");
  }

  async start(): Promise<void> {
    try {
      await reauthAccount(this.name, (event) => {
        if (event.event === "device_code") {
          this.showDeviceCode(event.verification_uri, event.user_code);
        }
      });
      if (this.finished) return;
      this.finished = true;
      this.showMessage(`Signed in as ${this.name}.`);
      this.tui.requestRender();
      this.done(true);
    } catch (error) {
      if (this.finished) return;
      this.finished = true;
      this.showMessage(error instanceof Error ? error.message : String(error));
      this.tui.requestRender();
      this.done(false);
    }
  }

  render(width: number): string[] {
    return this.container.render(width);
  }

  invalidate(): void {
    this.container.invalidate();
  }

  handleInput(data: string): void {
    if (this.finished) return;
    if (this.keybindings.matches(data, "tui.select.cancel")) {
      this.finished = true;
      this.done(false);
    }
  }

  private showDeviceCode(uri: string, userCode: string): void {
    const clickHint =
      process.platform === "darwin" ? "Cmd+click to open" : "Ctrl+click to open";
    const linkedUrl = `\x1b]8;;${uri}\x07${uri}\x1b]8;;\x07`;
    const linkedHint = `\x1b]8;;${uri}\x07${clickHint}\x1b]8;;\x07`;
    this.body.clear();
    this.body.addChild(new Spacer(1));
    this.body.addChild(new Text(this.theme.fg("accent", linkedUrl), 1, 0));
    this.body.addChild(new Text(this.theme.fg("dim", linkedHint), 1, 0));
    this.body.addChild(new Spacer(1));
    this.body.addChild(
      new Text(this.theme.fg("warning", `Enter code: ${userCode}`), 1, 0),
    );
    this.body.addChild(new Spacer(1));
    this.body.addChild(
      new Text(this.theme.fg("dim", "Waiting for authorization..."), 1, 0),
    );
    this.body.addChild(
      new Text(this.theme.fg("dim", "(esc to cancel)"), 1, 0),
    );
    this.tui.requestRender();
  }

  private showMessage(message: string): void {
    this.body.clear();
    this.body.addChild(new Spacer(1));
    this.body.addChild(new Text(this.theme.fg("text", message), 1, 0));
    this.body.addChild(new Spacer(1));
  }
}

export { AUTO_VALUE };
