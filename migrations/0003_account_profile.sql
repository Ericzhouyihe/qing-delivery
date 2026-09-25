-- 004 账号资料:头像外链与本地备注(昵称复用 display_name)
ALTER TABLE accounts ADD COLUMN avatar_url TEXT;
ALTER TABLE accounts ADD COLUMN remark TEXT;
