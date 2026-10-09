# 无 Continuo 服务端的同步

用户选择自己的 GitHub 专用私有仓库。首版使用本机 Git 认证，无需 Continuo OAuth 服务。开发测试允许本地 bare Git 仓库。

```text
refs/heads/continuo-sync
  manifest.json             格式版本与密钥标识
  events/<revision>.json    独立加密的不可变事件
```

Git 提交 SHA 是存储版本，事件 revision 和 parents 是业务因果关系，两者职责不同。客户端读取该分支，验证所有文件模式、路径、体积、manifest 与加密事件，再在单一 SQLite 事务中导入。随后将本地缺失事件打包为一个提交，以普通 fast-forward push 发布。push 被拒绝时保留本地事件，下一次重新读取远端再尝试；永不 force push。

两个设备修改不同对象可以同时保留。修改同一对象时，如果两个事件引用相同父版本，两个 head 都保留。合并时产生同时引用这两个 head 的新事件。时钟漂移不改变判定，重复同步依靠 revision 去重。相同 revision 携带不同内容会拒绝导入。

删除产生 tombstone。已知该删除的设备不会用旧版本覆盖它；离线设备基于删除前版本继续编辑则形成“删除与编辑冲突”，需要显式解决。Git 历史仍保留此前的密文；当前无自动历史压缩、数据擦除和旧设备重建协议。

加密：每个事件使用 32 字节随机工作空间密钥与随机 96-bit nonce，采用 RustCrypto ChaCha20-Poly1305。AAD 绑定协议版本、密钥标识与事件 revision。密钥文件不上传；新设备从安全的独立渠道获得它。manifest 保留 key fingerprint，事件名称/内容/设备标识在密文中。存储仍可观察事件数量、随机 revision 和 Git 提交时间。

本机 SQLite 保留明文业务记录；在 Unix 上目录权限 700、数据库和密钥权限 600。当前未集成 OS keychain 或 SQLite 静态加密，Windows ACL 尚需实现。凭据字段的校验只防止已知结构化秘密字段，不是自由文本秘密检测。

威胁边界：工作空间内持有同一密钥的设备互相信任。认证过的加密事件不等于独立设备签名。当前无密码学设备撤销或密钥轮换。0.7 加入本机已验证事件集合检查：同一地址/密钥下，曾观察到的事件消失时阻止导入与发布。只能保护本机已确认的历史，不能识别首次连接前、清空元数据后或更换地址/密钥的历史关系；不是设备签名和完整的恶意存储防护。GitHub token 撤销也不是密码学设备撤销。

文件上限：单个远端事件 blob 2 MiB，API 输入 1 MiB，远端最多 50,000 个协议文件。当前每轮会读取全部历史，适合验证小型工作空间，正式发布前需增加增量读取与压缩。同步为手动触发，批量提交，不承担高频消息流、自动远程执行或工作目录文件同步。

不 checkout 远端树，只读取和落盘通过协议校验的普通文件；拒绝符号链接、未知路径与非 blob 内容。工作目录位于应用私有临时目录，结束后清理。日志不回显 Git 认证诊断或远端凭据。

GitHub 参考：[非强制分支更新](https://docs.github.com/en/rest/git/refs#update-a-reference)、[速率限制](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)。首版实际使用 Git transport，不使用 Contents API 逐文件提交。

## 0.7 同步状态与安全重试

sync.inspect 为本机只读检查，返回配置版本、密钥状态、当前作业、并发冲突、删除数量、待上传/拉取。未验证过远端时数量为 null，界面显示“待检查”；之后基于最近已验证快照，离线时不声称实时状态。

sync.preview 连接已配置存储，校验协议、解密、完整因果关系和检查点，只写本机作业/检查点元数据，不导入业务事件、不建远端分支、不提交或发布。仍需写与网络授权。sync.run 先完成相同校验，再原子导入、加密、普通 fast-forward push。每次使用新 canonical UUID，同一 run_id 不重复启动，可先查询 sync.job。

阶段为 checking/contacting/fetching/validating/importing/encrypting/publishing；最终状态 succeeded/failed/cancelled/timeout/interrupted。进程退出后未完成作业读取时标记 interrupted。sync.cancel 请求取消当前作业；MCP/Web worker 执行传输，主循环仍响应取消。传输和加密记录间检查取消/超时，SQLite 原子校验和导入事务之间检查，不强行打断事务。默认 30 秒，可指定 100..90000 ms。

Git 使用本机认证，不回显原始认证诊断，不交互询问密码，不启用 hooks 或 checkout/smudge，输出与执行时间受限。macOS/Linux 尝试清理当前 Git 进程组；逃逸后台进程、强制关闭、系统故障不保证清理。Windows 的有界传输目前明确不支持。

取消/超时/断网不能撤回已发布远端提交，publication 可能为 unknown。已原子导入的验证事件保留，本机修改不回滚或覆盖。重试使用新 UUID，重新 fetch/校验/去重后正常 push；发布竞争失败返回 sync_push_failed，永不 force push。密钥缺失/替换/权限开放、密文损坏、未来协议、因果缺失均阻止部分导入；曾确认事件消失返回 sync_remote_history_missing，先从可信备份修复存储，不提供“忽略并覆盖”。

sync.configure 可传 expected_config_revision 做本机配置 CAS。sync.set_enabled 必须匹配配置版本；停用/恢复只影响本机，不擦除、不撤销其他设备、不轮换密钥。旧客户端不识别此本机标记，暂停不是不可绕过的权限边界。

## 删除与恢复

entity.history {id} 返回最近 200 条反向因果拓扑历史引用，并发版本以 UUID 作稳定次序；时间仅显示。更早已知 revision 可用 entity.history_version {id,revision} 读取，不能跨对象使用。

entity.restore {id,expected_revision,source_revision} 要求当前唯一且精确匹配的 tombstone，源属于该对象且未删除。克隆所选历史内容，生成以当前 tombstone 为 parent 的新版本，保留旧删除与编辑。关联身份/任务/能力重新校验，失效关联先修复。并发删除/编辑先显式整理全部 heads，不能跳过冲突。

恢复不包括原生 agent 内部日志、本机路径、账号认证、映射或丢失的密钥。本机明文数据库和密钥仍需自行备份。密钥生成用于新工作区，永不覆盖旧文件，不能解开旧密文。备份/密钥恢复向导、密码学撤销、轮换、自动和增量同步尚未实现。
