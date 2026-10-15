import { describe, expect, it } from "vitest";
import {
  commitHeader,
  headerTaken,
  withoutHeader,
} from "./hookHeaders";

/** 几次提交都在这个上面做，确认它**没被原地改**。 */
const stored = { Authorization: "Bearer abc" };

describe("headerTaken", () => {
  it("头名不区分大小写", () => {
    expect(headerTaken(stored, "authorization")).toBe(true);
    expect(headerTaken(stored, "AUTHORIZATION")).toBe(true);
  });

  it("正在改的那一行不算自己挡自己", () => {
    expect(headerTaken(stored, "Authorization", "Authorization")).toBe(false);
    // 只差大小写的改名也是「自己这一行」—— 允许改拼写
    expect(headerTaken(stored, "authorization", "Authorization")).toBe(false);
  });

  it("空名字永远不冲突（那一行还没填）", () => {
    expect(headerTaken(stored, "")).toBe(false);
    expect(headerTaken(stored, "   ")).toBe(false);
  });

  it("手写配置里的重复键能被发现", () => {
    const messy = { "X-Token": "a", "x-token": "b" };
    expect(headerTaken(messy, "X-Token", "x-token")).toBe(true);
  });
});

describe("commitHeader", () => {
  it("改名：删旧键、插新键、值跟着走", () => {
    const result = commitHeader(stored, "Authorization", {
      name: "X-Token",
      value: "Bearer abc",
    });
    expect(result).toEqual({
      headers: { "X-Token": "Bearer abc" },
      changed: true,
    });
    expect(stored).toEqual({ Authorization: "Bearer abc" }); // 没原地改
  });

  it("只改值：键留在原位", () => {
    const result = commitHeader(stored, "Authorization", {
      name: "Authorization",
      value: "  Bearer xyz  ",
    });
    expect(result).toEqual({
      headers: { Authorization: "Bearer xyz" },
      changed: true,
    });
  });

  it("名字和值都没变 = 不必回写设置", () => {
    // 「点一下输入框又点回来」是这条路径上最常见的一次，不该白跑一趟 set_settings
    expect(
      commitHeader(stored, "Authorization", {
        name: "Authorization",
        value: "Bearer abc",
      }),
    ).toEqual({ headers: stored, changed: false });
  });

  it("新加一行", () => {
    expect(commitHeader(stored, null, { name: " X-Token ", value: " 1 " })).toEqual({
      headers: { Authorization: "Bearer abc", "X-Token": "1" },
      changed: true,
    });
  });

  it("新加的一行还没填名字 = 等于没加（不回写设置）", () => {
    expect(commitHeader(stored, null, { name: " ", value: "1" })).toEqual({
      headers: stored,
      changed: false,
    });
  });

  it("把名字清空 = 删掉这一行", () => {
    expect(commitHeader(stored, "Authorization", { name: "", value: "" })).toEqual({
      headers: {},
      changed: true,
    });
  });

  it("和别的行撞名 = 拒绝（返回 null），让界面去报错", () => {
    const headers = { Authorization: "a", "X-Token": "b" };
    expect(
      commitHeader(headers, "X-Token", { name: "authorization", value: "b" }),
    ).toBeNull();
    // 只差大小写地改自己那一行是允许的（还是同一个头，只是换了拼写）
    expect(
      commitHeader(headers, "X-Token", { name: "x-token", value: "b" }),
    ).toEqual({ headers: { Authorization: "a", "x-token": "b" }, changed: true });
  });

  it("没改名的那一行不查重：手写配置里本来就有重复键时也能正常改值", () => {
    // `BTreeMap` 那边不合并只差大小写的键（见 hookHeaders.ts 顶部的说明），所以这种
    // 配置是能存进来的。此时「点一下输入框又点回来」不该弹一句「这个名字已经有了」。
    const messy = { "X-Token": "a", "x-token": "b" };
    expect(
      commitHeader(messy, "X-Token", { name: "X-Token", value: "a" }),
    ).toEqual({ headers: messy, changed: false });
  });
});

describe("withoutHeader", () => {
  it("删一行，不动别的，也不原地改", () => {
    const headers = { Authorization: "a", "X-Token": "b" };
    expect(withoutHeader(headers, "Authorization")).toEqual({ "X-Token": "b" });
    expect(headers).toEqual({ Authorization: "a", "X-Token": "b" });
  });
});
