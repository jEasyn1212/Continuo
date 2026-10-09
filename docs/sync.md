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

威胁边界：工作空间内持有同一密钥的设备互相信任。认证过的加密事件不等于独立设备签名。当前无设备撤销/密钥轮换、远端回滚检测或删除已提交事件的检测，不能把 GitHub token 撤销当成完整的密码学设备撤销。下一阶段需要相应模型和审计。

文件上限：单个远端事件 blob 2 MiB，API 输入 1 MiB，远端最多 50,000 个协议文件。当前每轮会读取全部历史，适合验证小型工作空间，正式发布前需增加增量读取与压缩。同步为手动触发，批量提交，不承担高频消息流、自动远程执行或工作目录文件同步。

不 checkout 远端树，只读取和落盘通过协议校验的普通文件；拒绝符号链接、未知路径与非 blob 内容。工作目录位于应用私有临时目录，结束后清理。日志不回显 Git 认证诊断或远端凭据。

GitHub 参考：[非强制分支更新](https://docs.github.com/en/rest/git/refs#update-a-reference)、[速率限制](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)。首版实际使用 Git transport，不使用 Contents API 逐文件提交。
