# rag-ferrite 运行镜像：二进制由 GitHub Actions 以 musl 静态方式预编译。
#
# 不在镜像内编译，好处是镜像层极小且可复现；编译过程见
# .github/workflows/build-musl.yml
#
# 依赖已确认全部静态化：
#   - rusqlite features=["bundled"]  → SQLite C 源码编进二进制
#   - sqlite-vec crate               → 静态链接，无需额外 .so
#   - reqwest features=["rustls-tls"] + webpki-roots → TLS 根证书内嵌，
#     所以 scratch 镜像不需要携带 /etc/ssl 证书。
FROM scratch

COPY ragfer /usr/local/bin/ragfer

# 配置文件查找顺序（src/config.rs:821-826）：先 ./config.toml，再
# $XDG_CONFIG_HOME/rag-ferrite/config.toml。把工作目录设为 /data，
# 就把 config.toml 落在挂载卷上，改配置不需要重建镜像。
WORKDIR /data

EXPOSE 4242

ENTRYPOINT ["/usr/local/bin/ragfer"]
CMD ["serve"]
