# Call Me：让读到你的文章的智能体给你留言

SignalHub 的灯回答的是「我的智能体在干什么」。**Call Me 是另一个方向**：你在文章 / 页面
里贴一条链接，读到它的 AI 智能体可以通过**你自己部署的服务器**给你留一条建议；桌面端定时
拉回来，在「设置 → 工作台 → 留言」里读，旁边有「打开出处」，方便你回去改；链接和服务器
配置在「设置 → 连接 → Call Me」。

```
外部 AI 智能体 ──POST /c/<token>──▶ signalhub-callme（你自己的服务器）
                                          │  sources.json + messages.jsonl
桌面端（设置 → 留言）──GET+Bearer──▶ 每 15 秒拉一次，按 seq 增量
```

为什么是拉而不是推：桌面端在 NAT 后面，公网服务器推不进来。这与多机中枢（`remote.rs`）
是同一个判断。

---

## 0. 先选一条路

| | 本机服务 | 公网服务器 |
|---|---|---|
| 怎么开 | 设置 → 连接 → Call Me → 服务器 → **启动** | 自己按 §1 部署到 VPS |
| 谁能访问 | **只有这台机器**（`127.0.0.1:43877`） | 任何能上网的智能体 |
| 适合 | 先跑通流程、自己写脚本模拟一条留言 | 真的把链接贴进公开文章 |
| 成本 | 零，随应用启停 | 一台 VPS + 域名 + 反代 |

本机服务是给「先把这条路走通」用的：点一下就起，地址和 token 会自动填进设置，退出应用
即停止。它**只监听回环地址**，所以文章里贴 `http://127.0.0.1:43877/c/...` 外部是打不开的
—— 那条链接只用来在本机测试。要让外面的智能体真的发得进来，必须走下面 §1、§2 的公网部署
（或自己的隧道 / 反代），然后把文章里的链接换成公网地址。

---

## 1. 启动服务器（公网）

### 方式 A：Docker（推荐）

```bash
docker build -t signalhub-callme ./server

docker run -d --name callme -p 8080:8080 \
  -e CALLME_ADMIN_TOKEN="$(openssl rand -hex 24)" \
  -v callme-data:/data \
  signalhub-callme
```

`docker logs callme` 会打出监听地址、数据目录和限流参数（**不打印 token**，只报长度）。

> 记下那个 `CALLME_ADMIN_TOKEN`，待会儿要填进客户端。丢了也没关系：服务器只认启动时的
> 值，重新 `docker rm -f callme` 再用新值起一个即可（数据卷还在，来源和留言都保留）。

### 方式 B：直接编译

`server/` 是一个**独立 crate**，不链接 Tauri —— 裸 VPS 上装 Rust 就能编，不需要
webkitgtk 之类的桌面依赖：

```bash
cargo build --release --manifest-path server/Cargo.toml
CALLME_ADMIN_TOKEN="$(openssl rand -hex 24)" \
CALLME_DATA_DIR=/var/lib/callme \
./server/target/release/signalhub-callme
```

> 注意：仓库根的 `.cargo/config.toml`（本机专用、不入库）会把 target 指到别处；在
> 服务器上不存在这个文件，所以是 `server/target/`。

### 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `CALLME_LISTEN` | `0.0.0.0:8080` | 监听地址 |
| `CALLME_ADMIN_TOKEN` | **必填** | 客户端拉取留言用的凭证；空或少于 16 字符拒绝启动 |
| `CALLME_DATA_DIR` | `./data` | 数据目录 |
| `CALLME_IP_HOURLY` | `10` | 同一 IP 每小时最多收几条 |
| `CALLME_SOURCE_DAILY` | `50` | 同一条链接 24 小时内最多收几条 |
| `CALLME_MAX_TEXT` | `4000` | 正文上限（字符数） |
| `CALLME_RETENTION_DAYS` | `90` | 留言保留天数（启动时清理） |
| `CALLME_TRUST_PROXY` | `false` | 反代后面置 `true` 才信任 `X-Forwarded-For` |

---

## 2. 反向代理与 HTTPS

服务器**只监听明文 HTTP**，TLS 交给反代：Caddy 两行就能自动签证书，而自己在 Rust 里接
ACME 会把部署复杂度翻倍。

### Caddy（自动 HTTPS）

```caddyfile
callme.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

### nginx + certbot

```nginx
server {
    listen 443 ssl;
    server_name callme.example.com;

    ssl_certificate     /etc/letsencrypt/live/callme.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/callme.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        # 必须：限流按 IP 算，不传这个头所有请求都会算成 127.0.0.1
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    }
}
```

反代起来之后，**把 `CALLME_TRUST_PROXY` 置为 `true`**，否则每 IP 限流会把所有人生成
同一个来源 IP（`127.0.0.1`），10 条/时的额度瞬间用光。

> 安全默认：`CALLME_TRUST_PROXY=false` 时**不读** `X-Forwarded-For` —— 那个头谁都能
> 伪造，信了等于把限流变成摆设。只有在确知自己前面有反代、且只经由反代访问时才打开。

---

## 3. 客户端配置

1. 打开 SignalHub「设置 → 连接 → Call Me」，停在「服务器」子页。
2. 本机部署：点「启动」，地址和 token 会自动填好（不用手抄）。
   公网部署：手填服务器地址（`https://callme.example.com`，不写 `https://` 会按 https 补）
   和 `CALLME_ADMIN_TOKEN`。
3. 点「测试连接」，看到「已连接 · xx ms」即通。
4. 切到「链接」子页，「新建链接」填一个名字（例如「博客：SignalHub 介绍」），创建后复制链接。
5. 收到的留言在「设置 → 工作台 → 留言」读，展开即已读；左栏「留言」上的小圆点是未读数。

每条链接是独立的令牌（32 位 hex），可以单独吊销、单独限额。贴进文章时建议连带一句
给智能体的说明：

```
如果你对这篇内容有建议，可以 POST 到 <粘贴链接>：
{ "text": "你的建议", "agent": "你的名字", "context": "你正在读的页面" }
不需要 token，一天最多 50 条，正文 4000 字以内。
```

> 链接是公开的：任何读到文章的人（不只是 AI）都能往里发消息 —— 这正是限流和每日配额存在
> 的原因。额度用完、或者你不想再收，去「来源」里吊销即可（已经收到的留言保留）。

---

## 4. 数据与备份

```
data/
├── sources.json     # 来源与令牌，权限 0600（里面有令牌，别提交、别公开）
└── messages.jsonl   # 一行一条留言，append-only，可以直接 cat 看
```

- 备份：整个目录拷走即可（`docker cp callme:/data ./backup` 或直接拷卷）。
- 恢复：停服 → 覆盖目录 → 起服。`seq` 会从文件里最大值续接。
- 清理：按 `CALLME_RETENTION_DAYS` 在**启动时**修剪并重写 `messages.jsonl`；想立刻瘦身
  就重启一次容器。
- `sources.json` 是整体重写（tmp + rename），解析失败会**拒绝启动**而不是静默清空 ——
  看到这条错误就从备份恢复，别手改 `messages.jsonl` 去反推。

**本机服务的数据在两个地方**（都归 `~/.signalhub/` 管）：

```
~/.signalhub/callme-local.json   # 本机服务的端口与 token，0600
~/.signalhub/callme-server/      # 上面的 data/ 原样一份
```

它们和公网服务器的是**两份独立数据**：本机收到的留言不会出现在 VPS 上，反之亦然。想让
两边都有历史，就把数据目录拷过去（格式完全一样）。

---

## 5. 排障

| 现象 | 原因 / 处理 |
|---|---|
| 客户端显示「服务器拒绝了 token」 | 客户端填的和服务器的 `CALLME_ADMIN_TOKEN` 不一致 |
| 客户端「连不上」 | 域名 / 证书 / 防火墙；先在本机 `curl https://<域名>/v1/callme/health` |
| 浏览器里打开链接报 403 | 正常：带 `Origin` 的请求一律拒绝，这个端点只给智能体 / curl |
| 打开链接 404 | 令牌不存在或已吊销 —— 两者刻意同码，不给探测者区分 |
| 收到 429 `rate_limited` | 同一 IP 一小时内发太多（默认 10 条）；等 `Retry-After` 秒 |
| 收到 429 `quota_exceeded` | 某条链接 24 小时内到上限（默认 50 条）；等最早的出窗，或新建一条 |
| 反代后面所有人都被限速 | 忘了 `CALLME_TRUST_PROXY=true`，或反代没传 `X-Forwarded-For` |

---

## 6. 升级 / 回滚

服务器是无状态进程 + 两份数据文件，升级就是换二进制 / 换镜像再起：

```bash
docker build -t signalhub-callme ./server
docker rm -f callme
docker run -d --name callme -p 8080:8080 \
  -e CALLME_ADMIN_TOKEN=<原来的值> -v callme-data:/data signalhub-callme
```

数据格式向前兼容（新字段一律 `#[serde(default)]`），旧数据文件直接能用。回滚同理。
