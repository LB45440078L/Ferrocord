-- Message Logger: remembers what deleted messages said.
--
-- Every incoming message is kept in a bounded in-memory cache. When Discord
-- reports that a cached message was deleted, its content is appended to a list
-- of the last MAX_DELETED deleted messages, which is persisted in storage.
--
--   /deleted [n]    log the last n deleted messages (default 10)
--   /deleted clear  forget all logged messages

local MAX_DELETED = 50
local CACHE_SIZE = 2000

local cache = {} -- message id -> { content, author, channel_id }
local order = {} -- message ids, oldest first
local deleted = ferrocord.storage.get("deleted") or {}

local function remember(message)
    if cache[message.id] == nil then
        table.insert(order, message.id)
        if #order > CACHE_SIZE then
            cache[table.remove(order, 1)] = nil
        end
    end
    local author = message.author or {}
    cache[message.id] = {
        content = message.content or "",
        author = author.global_name or author.username or "unknown",
        channel_id = message.channel_id,
    }
end

local function on_deleted(id)
    local entry = cache[id]
    if entry == nil then
        return false
    end
    cache[id] = nil
    table.insert(deleted, {
        id = id,
        channel_id = entry.channel_id,
        author = entry.author,
        content = entry.content,
        deleted_at = os.time(),
    })
    while #deleted > MAX_DELETED do
        table.remove(deleted, 1)
    end
    return true
end

ferrocord.on("MESSAGE_CREATE", remember)

ferrocord.on("MESSAGE_UPDATE", function(message)
    local entry = cache[message.id]
    if entry and type(message.content) == "string" then
        entry.content = message.content
    end
end)

ferrocord.on("MESSAGE_DELETE", function(event)
    if on_deleted(event.id) then
        ferrocord.storage.set("deleted", deleted)
    end
end)

ferrocord.on("MESSAGE_DELETE_BULK", function(event)
    local changed = false
    for _, id in ipairs(event.ids or {}) do
        changed = on_deleted(id) or changed
    end
    if changed then
        ferrocord.storage.set("deleted", deleted)
    end
end)

ferrocord.command("deleted", "Show recently deleted messages: [count] | clear", function(_channel_id, args)
    if args == "clear" then
        deleted = {}
        ferrocord.storage.set("deleted", deleted)
        ferrocord.log("cleared deleted message log")
        return
    end
    local count = math.clamp(tonumber(args) or 10, 1, MAX_DELETED)
    if #deleted == 0 then
        ferrocord.log("no deleted messages logged yet")
        return
    end
    local first = math.max(1, #deleted - count + 1)
    for i = first, #deleted do
        local d = deleted[i]
        ferrocord.log(string.format("[%s] %s in <#%s>: %s", os.date("%H:%M:%S", d.deleted_at), d.author, d.channel_id, d.content))
    end
end)
