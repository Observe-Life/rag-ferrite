# 【必读】rag-ferrite 使用指南

## 目录

- 一、最高指导
  - （一）指导原则的修改权限
  - （二）术语定义与一些规范
- 二、镜像构建
  - （一）构建方式与产物形态
  - （二）工作流清单
  - （三）三条必须知道的构建约束
- 三、容器部署
  - （一）目录结构
  - （二）compose 要点
  - （三）三条部署铁律
- 四、首次配置
  - （一）准备文件
  - （二）填三处密钥
  - （三）必改项
  - （四）验证清单
- 五、日常使用
  - （一）查询与状态
  - （二）入库必须用批量端点
  - （三）返回条数受 `reranker.top_k` 截断
  - （四）中文整句曾跳过重排（已修复）
  - （五）上下文增强已实测否决，保持关闭
  - （六）实测基线
- 六、源码修改
  - （一）本仓库相对上游的八项改动
  - （二）切词的作用范围与代价
  - （三）索引一致性的正确口径
- 七、已知缺陷
  - （一）`collection_tags` 建表语句冲突
  - （二）`Audit dependencies` 恒红

---

## 一、最高指导

### （一）指导原则的修改权限

> **警告**：该部分内容作为整篇文档、整个项目的指导原则，仅可由用户修改，AI禁止主动修改，但可以提建议让用户授权修改。

### （二）术语定义与一些规范

本RAG软件采用的混合检索方案：
1. 关键词检索：分词（双字滑窗、单字兜底的中、英文分词算法，入库生成倒排索引和检索时调用）、生成倒排索引（FTS5）、关键词检索（FTS5）、检索相关性打分（FTS5内置的BM25）
2. 语义检索：生成向量（云端API，为入库文本和搜索词句生成向量）、检索向量（KNN，用搜索词句的向量检索库存的向量）
3. 混合检索：多路结果融合算法（RRF）、重排（云端API）
4. 切块文本、倒排索引、向量的存储数据库：SQLite

---

## 二、镜像构建

### （一）构建方式与产物形态

**全部在 GitHub Actions 上完成，本机不编译任何东西。** 与 celld 不同，本项目改了 Rust 源码，所以镜像是自己构建的。

构建产物是 `FROM scratch` + 一个静态二进制，没有 libc 依赖：`rusqlite` 用 `bundled`（SQLite 源码编进二进制）、`sqlite-vec` 静态链接、TLS 根证书由 `webpki-roots` 内嵌，所以镜像里不需要 `/etc/ssl`。

产物推送到 `ghcr.io/observe-life/rag-ferrite:canary`，镜像约 21.6 MB。

### （二）工作流清单

| 工作流 | 触发 | 作用 |
|---|---|---|
| `build-musl.yml` | 手动 / 打 `v*` 标签 | 交叉编译 musl 静态二进制 → 构建 scratch 镜像 → 推 GHCR |
| `fmt.yml` | 仅手动 | 跑 `cargo fmt --all` 并自动提交结果 |
| `ci.yml`（上游） | push / PR | 格式、clippy、测试、依赖审计 |

### （三）三条必须知道的构建约束

1. **镜像构建只认手动触发和 `v*` 标签**。`build-musl.yml` 的触发器是 `workflow_dispatch` + `push:tags:v*`，普通 push 到 main 不会构建镜像；历次运行 `event` 均为 `workflow_dispatch` 可佐证。而 `ci.yml`（Rust quality gates）对每次 push 到 main 都跑，已配置 `paths-ignore` 排除 `**.md`、`docs/**`、`LICENSE`、`.gitignore`，但**不排除** `.github/workflows/**`——改 CI 配置本身必须被验证。

2. **镜像标签必须带 `ghcr.io/` 前缀**。漏掉前缀 buildx 会当成 Docker Hub 处理，拿 GitHub 令牌去 `auth.docker.io` 认证，报 `401 insufficient scopes`——看起来像权限问题，实际是笔误。工作流里已加自检，标签不含前缀直接失败。

3. **musl 需要显式指定交叉编译器**。`CC_x86_64_unknown_linux_musl=musl-gcc`，因为 `rusqlite` 的 bundled 与 `sqlite-vec` 都要编 C。

---

## 三、容器部署

### （一）目录结构

```
/vol1/docker/rag-ferrite/
├── docker-compose.yml
├── .env                      # 密钥，权限 600，不进 git
└── data/                     # 挂载进容器 = /data
    ├── config.toml           # 实际配置（含密钥，不进 git）
    ├── rag.sqlite3           # 数据库（含 -wal/-shm）
    └── library/              # 待入库文档
```

### （二）compose 要点

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

### （三）三条部署铁律

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

## 四、首次配置

### （一）准备文件

```bash
mkdir -p /vol1/docker/rag-ferrite/data/library
cp deploy/config.example.toml /vol1/docker/rag-ferrite/data/config.toml
```

### （二）填三处密钥

| 用途 | 写在哪 | 说明 |
|---|---|---|
| 向量化 | `.env` 的 `EMBEDDING_API_KEY` | 程序读环境变量（`src/embedding.rs:45-46`，也接受 `OPENAI_API_KEY`） |
| 重排 | `data/config.toml` 的 `reranker.api_key` | **只能写在文件里**，`src/main.rs:335-338` 用 `.expect()` 强制要求，无环境变量回退 |
| 服务访问口令 | `.env` 的 `RAG_API_KEY` | 随机生成即可；非 loopback 绑定时**必须有**，否则启动直接失败（`src/api.rs:513-541`） |

`data/config.toml` 和 `.env` 都含密钥，**都不要提交进仓库**。

### （三）必改项（模板已按本机设好，换环境要核对）

| 配置 | 值 | 不改的后果 |
|---|---|---|
| `http_bind_address` | `"0.0.0.0"` | 默认 `127.0.0.1`，只有容器自己能访问 |
| `allowed_hosts` | 必须含 `rag-ferrite`、`rag-ferrite:4242` | 程序只会自动追加 bind 地址本身，**不会**追加容器名。celld 用「容器名:端口」访问时 Host 头不在白名单，请求被拒 |
| `web_ui_enabled` | `true` | 默认 `false`，内置控制台不开放 |
| `reranker.base_url` | 完整地址 `.../v1/rerank` | 这是**完整 URL 不是前缀**，漏 `/rerank` 会 404 |
| `embedding.base_url` | 前缀 `.../v1` | 与上面相反，程序自动补 `/embeddings` |
| `reranker.top_k` | 按需（本机 10） | 决定重排后最多返回几条，见五（三） |
| `llm.context_enabled` | `false` | 上游默认 `true`，入库时每个父块调一次大模型；无可用 LLM 会报 `ingestion will not work`。**另注意：`bc80ba8` 之前的镜像开启此项会导致入库 100% 死锁**（见六（一）） |

### （四）验证清单

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
| 全新查询 `limit=20` | 返回条数等于 `reranker.top_k`，且每条带 `rerank_score` |

---

## 五、日常使用

### （一）查询与状态

查询走 `POST /api/query`，带 `Authorization: Bearer <RAG_API_KEY>`。全新查询实测 0.67–0.71 秒（含重排）。

两种访问地址：**容器之间**用 `http://rag-ferrite:4242`（容器名:机内端口，不需要端口映射）；**浏览器和宿主机**用 `http://<服务器IP>:18081`。

### （二）入库必须用批量端点，不要用逐篇 `/api/ingest/data`

`/api/ingest` 接受 `{"paths":[...]}`，走 `IngestJob::Batch`，**整批只做一次索引 flush**；而逐篇 `/api/ingest/data` 每篇都会触发一次 flush。文件须位于 `allowed_ingest_roots`（本机为 `/data/library`）之下。内容按 SHA-256 判重，重复提交会被跳过（回执里 `0 chunks`），所以重跑同一批是安全的。

> `/api/ingest` **不能直接传目录**：传目录不报错，而是当单文件处理、返回 `total_files: 1` 后静默失败。要递归展开成文件列表再提交。

### （三）返回条数受 `reranker.top_k` 截断

`top_k = 10` 时，即使请求 `limit=20`，全新查询也只返回 10 条。要拿更多重排结果，必须同时调大 `reranker.top_k`，不能只调 `limit`。

### （四）中文整句曾跳过重排（已修复）

`classify_query` 用 `split_whitespace().count()` 数词，中文整句没有空格恒等于 1，于是被 `simple_word_threshold = 2` 判成 Simple 路径——Simple **不重排、也不受 `reranker.top_k` 截断**。实测（`limit=20`，加一次性 nonce 保证全新）：带空格的查询返回 10 条且每条带 `rerank_score`（最高 0.99），纯中文整句返回满 20 条且 `rerank_score` 全空，容器日志直接印 `complexity=Simple`。

曾据这个现象误判为"结果缓存会绕过重排"。**真因不是缓存**：同一句查询重复执行命中缓存，返回的正是同一份**已重排**结果（实测带分、0.01 秒）。当时看到"重复查询返回 20 条无分"，是因为那句查询本身就走的 Simple。

已由 `01f9c66` 修复（分类改按语义单元计数，见六（一））。修复后 6 条中文查询全部走重排。

**做检索质量评测仍必须用全新查询**（句子里加一次性 nonce 即可），否则不同轮次命中的是缓存里的同一份结果，不可比。

### （五）上下文增强（`llm.context_enabled`）已实测否决，保持关闭

| 层面 | 关闭 | 开启 |
|---|---|---|
| 文档级 Hit@1（13 条查询） | 12/13 | 12/13（名次零变化，失败点相同） |
| 块级 MRR（10 条 gold，宽松口径） | **0.867** | 0.803 |

机制：上下文前缀对同一文档的所有块近似，把组内相似度从 0.4801 抬到 0.5091，代价是**抹平块与块的差异**——精确细节类查询里正确块的绝对相似度反而下降（0.5436→0.4746），与相邻块的差距从 0.048 缩到 0.001。检索最终按块返回，故净效果为负。它优化的是"哪篇文档"，损害的是"哪一段"。

附加成本：入库慢 10 倍以上（每批 9–20 秒 vs 0.6–1.6 秒），每 10 篇约 90 万 token；批量调用首次失败率高，靠 `context_max_retries=3` 兜住，**调低会静默丢上下文**。

提示词只送文档前 8000 字符（`max_document_prompt_chars`），实测对长文后部块的上下文质量影响很小——模型主要依据 `<chunk>` 自身推断。

### （六）实测基线

当前库规模（2026-11-05 实测）：

| 指标 | 数值 |
|---|---|
| 文档 | 203 篇 |
| 块 | 12,615（其中 parent 1,732 不进全文索引） |
| 全文索引 | 10,883 行，缺失 0、孤儿 0 |
| 库文件 | 173 MB |
| 常驻内存 | 13.2 MiB |
| 全新查询耗时 | 0.67–0.71 秒（含重排） |

历史性能结论（与规模无关，仍有效）：

| 指标 | 数值 | 测于 |
|---|---|---|
| 批量导入 | 40 篇 / 1596 块 / 63 秒 / 0 错误（约 1.6 秒/篇） | 80 篇规模时 |
| 全量索引重建 | 5652 行 5.0 秒（事务化前 2724 行约 660 秒） | 80 篇规模时 |

---

## 六、源码修改

### （一）本仓库相对上游的八项改动

| 提交 | 内容 | 为什么 |
|---|---|---|
| `a1432ea` | 消除混合检索与关键词检索的**嵌套取锁死锁** | 全进程只有一个 SQLite 连接由 `std::sync::Mutex` 保护，而该锁不可重入；`search_hybrid` 外层持锁后又调用内层取锁的子函数，请求永久挂起。修法采用项目自身已有的块作用域范式 |
| `ba10e38` | **FTS5 MATCH 表达式转义** | `.` `/` 在 FTS5 里是操作符，`clippy.toml`、`crates/logic/lib.rs` 这类查询会报语法错误并冒泡成 HTTP 500 |
| `fd71320` | **中文切词**（`src/storage/fts_text.rs`） | FTS5 的 `unicode61` 把连续汉字当成一个词，中文关键词检索必然落空。规则：汉字段出单字+相邻双字；ASCII 段整词小写并保留 `- _ . / +`；中英紧邻补跨界词；标点断开。索引侧与查询侧共用同一函数 |
| `4359a2d` | `default_log_filter()` 由 `rag_ferrite=debug` 改为 `info` | crate 真名是 `ragfer`，名字不匹配导致所有日志被过滤，容器表现为毫无输出 |
| `bc80ba8` | 消除**入库路径**的嵌套取锁死锁 | `commit_parent_to_db` 的 guard 作用域直到函数末尾，而标签分支里又二次 `get_conn()`。该分支只在上下文增强产出 TAGS 时进入，因此关闭 `context_enabled` 时永不触发——这是它长期未被发现的原因。实测开启后入库 100% 卡死：18 线程全停在 `futex_wait`、无一停在 `epoll_wait`、无对外连接 |
| `4137990` | 队列层不再对**单篇任务**无条件全量重建索引 | `ingestion.rs` 的 `IngestJob::File`/`Data` 分支在每个任务后都调 `rebuild_indexes_serialized()`，**完全绕过 `defer_index_rebuild`（默认已是 `true`）**。于是每篇一次全库重建，逐篇导入累计成本 O(n²)，且重建持有全局单连接锁、期间所有 API 无响应。改为尊重该配置；`Batch` 分支与显式 flush 保留（那才是 `flush_indexes()` 注释声明的意图） |
| `01f9c66` | 查询分类改按**语义单元**计数（`pipeline::term_count`） | `classify_query` 原用 `split_whitespace().count()` 数词，中文整句没有空格恒等于 1，被 `simple_word_threshold = 2` 判成 Simple 路径，而 Simple **不重排、也不受 `reranker.top_k` 截断**——等于所有中文查询都没有重排。新口径：ASCII 仍按空白计数，含汉字的段按相邻双字向上取整，段内 ASCII 子串各计一个；汉字判定复用 `fts_text::is_ideograph`，与切词共用同一套定义。实测：修复前 6 条中文查询一律返回满 20 条且 `rerank_score` 全空；修复后一律返回 10 条且每条带分（0.63~0.9995），单查询多约 0.3~0.6 秒。详见五（四） |
| `32aac94` | 索引重建整体包进**一个事务** | 逐行 `stmt.execute()` 在 WAL 下每行一个隐式事务，而 FTS5 每次插入都要更新倒排 b-tree 并 segment merge，成本随索引规模上升——实测 2724 行重建约 660 秒。事务化后 5652 行只用 5.0 秒。同一事务还保证 DELETE 与重插原子：此前进程被杀会留下「索引已清空大半」的残缺状态且重启不自愈 |

### （二）切词的作用范围与代价

切词只作用于全文索引表 `chunks_fts`；`chunks.content` 保持原文，因此重排模型和大模型读到的仍是干净文本。代价是 OR 连接使召回变宽，精度依赖 `bm25()` 打分、RRF 与重排收敛。

### （三）索引一致性的正确口径

**判断全文索引是否完整，不能拿 `chunks` 总数减 `chunks_fts` 行数。** 父块（`chunk_type='parent'`）只提供上下文、不参与检索，本来就不写进 FTS。

正确口径：

```sql
SELECT COUNT(*) FROM chunks WHERE chunk_type != 'parent';   -- 应索引
SELECT COUNT(*) FROM chunks_fts;                              -- 实际
```

2026-11-05 实测：应索引 10,883 = 实际 10,883，缺失 0、孤儿 0。按错误口径会得出「缺 1,732 行」的假缺口——这个误判在本项目上发生过两次。

---

## 七、已知缺陷

### （一）`collection_tags` 建表语句冲突

`engine/mod.rs:211` 建出的表是 `(tag PRIMARY KEY, collection)`，而 `engine/tags.rs:27` 的建表与写入都假定还有 `chunk_count` 列。`CREATE TABLE IF NOT EXISTS` 让先执行的赢，于是标签聚合 SQL 必然失败，表现为每次查询都打一条 `WARN Tag routing failed: no such column: chunk_count`。功能降级但不崩，标签路由（按标签圈定素材范围）实际不可用。修它要改 schema 并涉及已有库迁移。

### （二）`Audit dependencies` 恒红

命中 32 条安全公告，原因是 fork 基线停在 2026-10-07，上游此后已 bump rmcp/tokio/toml/ratatui。与本仓库改动无关（未引入任何依赖）。

---
*最后更新：2026-11-06。*
