# SignalHub Call Me 服务器镜像。
#
# 构建：docker build -t signalhub-callme .
# 运行：docker run -d --name callme -p 8080:8080 \
#         -e CALLME_ADMIN_TOKEN=<openssl rand -hex 24> \
#         -v callme-data:/data signalhub-callme
#
# 两阶段构建：构建阶段要完整的 rust 工具链 + C 链接器（axum/tokio 是纯 Rust，
# 但仍要 cc 做链接）；运行阶段只要一个 glibc 基础镜像，**不带任何 Tauri /
# webkit 依赖** —— 这正是服务器独立成 crate 的原因。

FROM rust:1.96-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# --locked：锁定依赖版本，保证镜像可复现（Cargo.lock 已入库）。
RUN cargo build --release --locked

FROM debian:bookworm-slim

# 非 root 运行。数据目录归它所有，宿主机挂载卷时注意属主。
RUN useradd --system --uid 10001 --home /data --shell /usr/sbin/nologin callme \
    && mkdir -p /data \
    && chown callme:callme /data

COPY --from=build /src/target/release/signalhub-callme /usr/local/bin/signalhub-callme

ENV CALLME_LISTEN=0.0.0.0:8080 \
    CALLME_DATA_DIR=/data

VOLUME ["/data"]
EXPOSE 8080

USER callme
ENTRYPOINT ["signalhub-callme"]
