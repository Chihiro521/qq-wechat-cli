# Classic QQ 9.7.25.29417 独立页解密

已使用固定合成密钥的客户端 DLL 对照与本机真实数据库验证。客户端 DLL 仅作为开发参考，Rust 核心没有链接或装载该 DLL。

## 格式

加密文件前 1024 字节为扩展头，起始为 `SQLite header 3\0`。其后按 8192 字节分块；每个分块是 2048 个小端 `u32`。16 字节密钥解释为 4 个小端 `u32`。

变换为 XXTEA / Corrected Block TEA，无逐页 IV，算术使用 `u32` 环绕。轮数为 `6 + 52 / n`，`n=2048` 时为 6。常量 delta 为 `0x9e3779b9`。解密去掉扩展头，首个明文页应包含标准 SQLite 文件头；整个产物继续执行 SQLite 完整性检查。

实现见 `native/src/crypto.rs` 的 `qq_block` 和 `decrypt_qq_delta`。增量时以每页密文 SHA-256 判定变化，先复制旧的成功明文缓存，替换变化页，并重新校验首个页面及 SQLite 结构。

## 大库锁页

本机真实库超过 1 GB 后，标准 SQLite 的 `0x40000000` pending byte 会把与 PCQQ 不同的页面视为锁页，出现交叉引用 / 未使用页面错误。PCQQ 的文件偏移包含扩展头，其明文视图对应 `0x3ffffc00`。对原始 QQ 缓存使用该位置后，同一解密产物完整性检查通过。

`sqlite_compat.rs` 通过作用域恢复 `SQLITE_TESTCTRL_PENDING_BYTE`。源库读取与统一索引操作使用各自的位置；引擎在一个线程内顺序处理 SQLite 工作。该兼容设置仅适用于受支持的 PCQQ 原始缓存，统一索引仍为标准 SQLite。

## 开发参考

受支持的 QQ.exe SHA-256：

`e20e6670f31472e4545af1b6115f1a5cc553bbd0da2894b8def41cc3280e2347`

KernelUtil.dll SHA-256：

`b39f79c6cf43a5665dabb587f31784c713fbfda55ef0ebdd4a51ac1d0a0ec4e9`

参考变换 RVA `0x90f78`，codec callback RVA `0x64107`。`research/qq-oracle.cpp` 必须构建为 x86，只接收合成密钥 `42` 重复 16 次；输入为原始整页明文，输出与 Rust 的加密字节逐字节比较。`native/tests/fixtures/qq-page.json` 为固定参考向量。

运行时只读取匹配版本进程中目标数据库的 codec 上下文，核对 pager / callback / key buffer 布局，再用实际首个加密页面验证。恢复后的密钥由 DPAPI 封装到独立状态目录，协议和日志不输出密钥。

`research/generate-fixtures.py` 生成合成的 QQ / 微信端到端夹具。微信页参考由 PyCryptodome 独立生成。`research/verify-real.py` 与 `compare-qq.mjs` 对同一成功快照核对旧解析器与新索引，仅输出汇总；真实中间记录必须留在被忽略的 `.local` 下。
