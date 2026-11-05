import { describe, expect, it } from "vitest";
import { buildOnboardingPrompt, type OnboardingContext } from "./onboardingPrompt";
import { REPORTABLE_SIGNALS } from "./types";

const base: OnboardingContext = {
  lang: "zh",
  builtin: ["claude-code", "codex", "cursor"],
  families: ["claude-code", "codex", "cursor", "gemini"],
  agentsFile: "/Users/me/.signalhub/agents.json",
  dataDir: "/Users/me/.signalhub",
  port: 8787,
};

describe("buildOnboardingPrompt", () => {
  it("把运行时的真值写进提示词，而不是抄一份常量", () => {
    const text = buildOnboardingPrompt(base);
    expect(text).toContain("claude-code / codex / cursor");
    expect(text).toContain("claude-code / codex / cursor / gemini");
    expect(text).toContain("/Users/me/.signalhub/agents.json");
    expect(text).toContain("/Users/me/.signalhub/hooks.log");
    expect(text).toContain("/Users/me/.signalhub/token");
    expect(text).toContain("http://127.0.0.1:8787/v1/state");
  });

  it("英文版同样带上真值，且用词是英文", () => {
    const text = buildOnboardingPrompt({ ...base, lang: "en" });
    expect(text).toContain("claude-code / codex / cursor / gemini");
    expect(text).toContain("/Users/me/.signalhub/agents.json");
    expect(text).toContain("http://127.0.0.1:8787/v1/state");
    expect(text).toContain("Help me connect to SignalHub");
    expect(text).not.toContain("帮我接入");
  });

  it("两种语言都给出三条路：内置直装、自定义预设、以及不需要 hook 格式的兜底", () => {
    for (const lang of ["zh", "en"] as const) {
      const text = buildOnboardingPrompt({ ...base, lang });
      expect(text).toContain("signalhub install hooks");
      expect(text).toContain("signalhub run --agent");
      expect(text).toContain("/v1/state");
      expect(text).toContain("signalhub diagnose");
      expect(text).toContain("--dry-run");
    }
  });

  it("写入走 add-agent 命令，而不是让智能体自己编辑配置文件", () => {
    for (const lang of ["zh", "en"] as const) {
      const text = buildOnboardingPrompt({ ...base, lang });
      expect(text).toContain("signalhub add-agent");
      for (const flag of ["--id", "--name", "--family", "--config", "--marker", "--command"]) {
        expect(text).toContain(flag);
      }
      // 命令的硬约束和幂等 / 覆盖语义都要写清楚，agent 才不会去猜
      expect(text).toContain("--replace");
      expect(text).toContain("--http");
      expect(text).toContain("config.json");
      expect(text).toContain("_example");
      expect(text).toContain(lang === "zh" ? "不要直接编辑" : "Do not edit");
      expect(text).toContain(lang === "zh" ? "不要绕过" : "do not bypass");
    }
  });

  it("运行时的数据目录写进 --config 约束，不给 agent 留猜测空间", () => {
    for (const lang of ["zh", "en"] as const) {
      const text = buildOnboardingPrompt({ ...base, lang });
      expect(text).toContain("/Users/me/.signalhub");
      expect(text).toContain("/Users/me/.signalhub/agents.json");
    }
  });

  it("数据还没从 IPC 回来时给指路文案，不编造清单、也不漏出 undefined", () => {
    const empty: OnboardingContext = {
      lang: "zh",
      builtin: [],
      families: [],
      agentsFile: null,
      dataDir: null,
      port: null,
    };
    for (const lang of ["zh", "en"] as const) {
      const text = buildOnboardingPrompt({ ...empty, lang });
      expect(text).not.toMatch(/undefined|null|\[\]/);
      expect(text).toContain("~/.signalhub/agents.json");
      expect(text).toContain("<port>");
      expect(text).toContain("token");
      // 清单没拿到时指回界面，而不是凭空列几家出来
      expect(text).toContain(
        lang === "zh" ? "以 SignalHub「接入」页列出的为准" : "see the list on SignalHub's Connect page",
      );
    }
  });

  it("过滤掉空名字，避免拼出 `a /  / b` 这种空清单项", () => {
    const text = buildOnboardingPrompt({
      ...base,
      builtin: ["claude-code", "", "  ", "codex"],
    });
    expect(text).toContain("claude-code / codex");
    expect(text).not.toContain("/  /");
  });

  it("Windows 路径用反斜杠拼子文件名，不混出半截斜杠", () => {
    const text = buildOnboardingPrompt({
      ...base,
      agentsFile: "C:\\Users\\me\\.signalhub\\agents.json",
      dataDir: "C:\\Users\\me\\.signalhub",
      port: null,
    });
    expect(text).toContain("C:\\Users\\me\\.signalhub\\agents.json");
    expect(text).toContain("C:\\Users\\me\\.signalhub\\hooks.log");
    expect(text).toContain("C:\\Users\\me\\.signalhub\\port");
    expect(text).not.toContain(".signalhub/hooks.log");
  });

  it("状态词直接取 REPORTABLE_SIGNALS，不另抄一份会漂移的表", () => {
    for (const lang of ["zh", "en"] as const) {
      const text = buildOnboardingPrompt({ ...base, lang });
      expect(text).toContain(REPORTABLE_SIGNALS.join(" / "));
      expect(text).toContain("thinking");
    }
  });

  it("只指向 token 文件，不把任何凭据明文写进这段会被粘贴的文本", () => {
    const text = buildOnboardingPrompt(base);
    expect(text).toContain("/Users/me/.signalhub/token");
    expect(text).not.toMatch(/Bearer [A-Za-z0-9_-]{8,}/);
    expect(text).not.toContain("?t=");
  });
});
