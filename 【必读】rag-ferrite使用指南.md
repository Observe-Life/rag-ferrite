# 【必读】rag-ferrite 使用指南

派生自 [lelabdev/rag-ferrite](https://github.com/lelabdev/rag-ferrite)（MIT），基线 commit `c54ec42`（v5.3.0）。
本仓库在之上做了四项改造：**修复两处上游缺陷**、**修正日志默认值**、**接入中文切词**。

| 位置 | 内容 |
|---|---|
| 镜像 | `ghcr.io/observe-life/rag-ferrite:canary`（约 21.6 MB） |
| 部署目录 | `/vol1/docker/rag-ferrite/` |
| 机内地址 | `http://rag-ferrite:4242` |
| 浏览器地址 | `http://<服务器IP>:18081` |

---

## 一、镜像构建

**全部在 GitHub Actions 上完成，本机不编译任何东西。**

| 工作流 | 触发 | 作用 |
|---|---|---|
| `build-musl.yml` | 手动 / 打 `v*` 标签 | 交叉编译 musl 静态二进制 → 构建 scratch 镜像 → 推 GHCR |
| `fmt.yml` | 仅手动 | 跑 `cargo fmt --all` 并自动提交结果 |
| `ci.yml`（上游） | push / PR | 格式、clippy、测试、依赖审计 |

构建产物是 `FROM scratch` + 一个静态二进制，没有 libc 依赖：`rusqlite` 用 `bundled`（SQLite 源码编进二进制）、`sqlite-vec` 静态链接、TLS 根证书由 `webpki-roots` 内嵌，所以镜像里不需要 `/etc/ssl`。

### 四条必须知道的构建约束

1. **镜像标签必须带 `ghcr.io/` 前缀**。漏掉前缀 buildx 会当成 Docker Hub 处理，拿 GitHub 令牌去 `auth.docker.io` 认证，报 `401 insufficient scopes`——看起来像权限问题，实际是笔误。工作流里已加自检，标签不含前缀直接失败。
2. **musl 需要显式指定交叉编译器**。`CC_x86_64_unknown_linux_musl=musl-gcc`，因为 `rusqlite` 的 bundled 与 `sqlite-vec` 都要编 C。
3. **Actions 自己推送的提交不会再触发工作流**（GitHub 防递归）。所以 `fmt.yml` 提交的格式化结果需要**补一个空提交**才能进 CI。
4. **GitHub 事故期间状态字段会滞后**。同一 job 可能同时显示 `completed/failure` 和 `queued/in progress`，日志也读不到。判断构建是否成功，**用拉取镜像比对 digest，不要信 `run list`**：

   ```bash
   OLD=$(docker inspect --format '{{.Id}}' ghcr.io/observe-life/rag-ferrite:canary)
   docker pull ghcr.io/observe-life/rag-ferrite:canary
   [ "$(docker inspect --format '{{.Id}}' ghcr.io/observe-life/rag-ferrite:canary)" != "$OLD" ] \
     && echo "新镜像已产出"
   ```

`Audit dependencies` 目前为红：命中 32 条安全公告，原因是 fork 基线停在 10-08，上游此后已 bump rmcp/tokio/toml/ratatui。与本仓库改动无关（未引入任何依赖）。

---

## 二、容器部署

目录结构（宿主机）：

```
/vol1/docker/rag-ferrite/
├── docker-compose.yml
├── .env                      # 密钥，权限 600，不进 git
└── data/                     # 挂载进容器 = /data
    ├── config.toml           # 实际配置（含密钥，不进 git）
    ├── rag.sqlite3           # 数据库（含 -wal/-shm）
    └── library/              # 待入库文档
```

`docker-compose.yml` 要点：

```yaml
services:
  rag-ferrite:
    image: ghcr.io/observe-life/rag-ferrite:canary
    container_name: rag-ferrite
    volumes: [ ./data:/data ]
    networks: [ backend ]          # 机内通信标准：容器名:端口
    ports: [ "18081:4242" ]        # 仅给浏览器用，容器间通信不需要
    environment:
      - EMBEDDING_API_KEY=${EMBEDDING_API_KEY}
      - RAG_API_KEY=${RAG_API_KEY}
networks:
  backend: { external: true }
```

### 三条部署铁律

1. **改 `.env` 或配置后必须 `up -d --force-recreate`，不能用 `restart`**。`restart` 只重启进程，**不会重新读取 `.env` 插值**，容器里带的还是旧值——曾因此让程序拿着占位串「待填」去请求 API，一直报 401。

   ```bash
   cd /vol1/docker/rag-ferrite && docker compose up -d --force-recreate
   ```

2. **scratch 镜像里没有 shell 和 coreutils**，所以不能 `docker exec`，compose 里也不能写 `exec` 形式的健康检查。要看状态用 HTTP：

   ```bash
   curl -H "Authorization: Bearer $(grep ^RAG_API_KEY= .env | cut -d= -f2-)" \
        http://127.0.0.1:18081/api/status
   ```

3. **数据必须放数据盘**。`/vol1` 是本地 btrfs 块设备，SQLite 的 WAL 才能安全使用；WAL 三件套（`.db`/`-wal`/`-shm`）不能跨文件系统或放网络盘。

---

## 三、首次配置

```bash
mkdir -p /vol1/docker/rag-ferrite/data/library
cp deploy/config.example.toml /vol1/docker/rag-ferrite/data/config.toml
```

### 1. 填三处密钥

| 用途 | 写在哪 | 说明 |
|---|---|---|
| 向量化 | `.env` 的 `EMBEDDING_API_KEY` | 程序读环境变量（`src/embedding.rs:45-46`，也接受 `OPENAI_API_KEY`） |
| 重排 | `data/config.toml` 的 `reranker.api_key` | **只能写在文件里**，`src/main.rs:335-338` 用 `.expect()` 强制要求，无环境变量回退 |
| 服务访问口令 | `.env` 的 `RAG_API_KEY` | 随机生成即可；非 loopback 绑定时**必须有**，否则启动直接失败（`src/api.rs:513-541`） |

`data/config.toml` 和 `.env` 都含密钥，**都不要提交进仓库**。

### 2. 必改项（模板已按本机设好，换环境要核对）

| 配置 | 值 | 不改的后果 |
|---|---|---|
| `http_bind_address` | `"0.0.0.0"` | 默认 `127.0.0.1`，只有容器自己能访问 |
| `allowed_hosts` | 必须含 `rag-ferrite`、`rag-ferrite:4242` | 程序只会自动追加 bind 地址本身，**不会**追加容器名。celld 用「容器名:端口」访问时 Host 头不在白名单，请求被拒 |
| `web_ui_enabled` | `true` | 默认 `false`，内置控制台不开放 |
| `reranker.base_url` | 完整地址 `.../v1/rerank` | 这是**完整 URL 不是前缀**，漏 `/rerank` 会 404 |
| `embedding.base_url` | 前缀 `.../v1` | 与上面相反，程序自动补 `/embeddings` |
| `llm.context_enabled` | `false` | 上游默认 `true`，入库时每个父块调一次大模型；无可用 LLM 会报 `ingestion will not work`。**另注意：`bc80ba8` 之前的镜像开启此项会导致入库 100% 死锁**（见下方改动表） |

### 3. 验证清单

```bash
KEY=$(grep ^RAG_API_KEY= /vol1/docker/rag-ferrite/.env | cut -d= -f2-)
Q() { curl -s -m 60 -X POST -H "Authorization: Bearer $KEY" -H 'Content-Type: application/json' \
        -d "{\"query\":\"$1\",\"limit\":5}" http://127.0.0.1:18081/api/query; }
```

| 检查 | 期望 |
|---|---|
| `/api/status` | `{"document_count":N,"version":"5.3.0"}` |
| 启动日志 | 出现 `Embedding provider: ...` 与 `Reranker: Cohere ...` |
| 查询 `clippy.toml` | HTTP 200（修复前是 500） |
| 中文查询返回里 `bm25_rank` | 有非零值（接入切词前恒为 0） |
| `limit=20` 查询 | 返回含 `rerank_score` 字段，且耗时明显高于 `limit=5` |

---

## 本仓库相对上游的五项改动

| 提交 | 内容 | 为什么 |
|---|---|---|
| `a1432ea` | 消除混合检索与关键词检索的**嵌套取锁死锁** | 全进程只有一个 SQLite 连接由 `std::sync::Mutex` 保护，而该锁不可重入；`search_hybrid` 外层持锁后又调用内层取锁的子函数，请求永久挂起。修法采用项目自身已有的块作用域范式 |
| `ba10e38` | **FTS5 MATCH 表达式转义** | `.` `/` 在 FTS5 里是操作符，`clippy.toml`、`crates/logic/lib.rs` 这类查询会报语法错误并冒泡成 HTTP 500 |
| `fd71320` | **中文切词**（`src/storage/fts_text.rs`） | FTS5 的 `unicode61` 把连续汉字当成一个词，中文关键词检索必然落空。规则：汉字段出单字+相邻双字；ASCII 段整词小写并保留 `- _ . / +`；中英紧邻补跨界词；标点断开。索引侧与查询侧共用同一函数 |
| `4359a2d` | `default_log_filter()` 由 `rag_ferrite=debug` 改为 `info` | crate 真名是 `ragfer`，名字不匹配导致所有日志被过滤，容器表现为毫无输出 |
| `bc80ba8` | 消除**入库路径**的嵌套取锁死锁 | `commit_parent_to_db` 的 guard 作用域直到函数末尾，而标签分支里又二次 `get_conn()`。该分支只在上下文增强产出 TAGS 时进入，因此关闭 `context_enabled` 时永不触发——这是它长期未被发现的原因。实测开启后入库 100% 卡死：18 线程全停在 `futex_wait`、无一停在 `epoll_wait`、无对外连接 |

切词只作用于全文索引表 `chunks_fts`；`chunks.content` 保持原文，因此重排模型和大模型读到的仍是干净文本。代价是 OR 连接使召回变宽，精度依赖 `bm25()` 打分、RRF 与重排收敛。

---

## 实测基线（10 篇 / 376 块）

| 指标 | 数值 |
|---|---|
| 常驻内存 | 2.6–3.6 MiB（随运行时长浮动，CPU 0%） |
| 库文件 | 8.6 MB + WAL 0.9 MB |
| 单查询耗时 | 0.17–0.79 秒 |
| Hit@3 | 13/13 |
| Hit@1 | 无重排 12/13；有重排 11/13 |

失败集中在**主题相近的文档互相混淆**（如《MSP简介》与《MSP前景分析》）。核查后修正：两篇各自都含定义式表述，属文档内容本身交叉，不是索引能区分的问题，该条不应判错。

**上下文增强（`llm.context_enabled`）已实测否决，保持关闭。**

| 层面 | 关闭 | 开启 |
|---|---|---|
| 文档级 Hit@1 | 12/13 | 12/13（名次零变化，失败点相同） |
| 块级 MRR（10 条 gold，宽松口径） | **0.867** | 0.803 |

机制：上下文前缀对同一文档的所有块近似，把组内相似度从 0.4801 抬到 0.5091，代价是**抹平块与块的差异**——精确细节类查询里正确块的绝对相似度反而下降（0.5436→0.4746），与相邻块的差距从 0.048 缩到 0.001。检索最终按块返回，故净效果为负。它优化的是"哪篇文档"，损害的是"哪一段"。

附加成本：入库慢 10 倍以上（每批 9–20 秒 vs 0.6–1.6 秒），每 10 篇约 90 万 token；批量调用首次失败率高，靠 `context_max_retries=3` 兜住，**调低会静默丢上下文**。

提示词只送文档前 8000 字符（`max_document_prompt_chars`），实测对长文后部块的上下文质量影响很小——模型主要依据 `<chunk>` 自身推断。
