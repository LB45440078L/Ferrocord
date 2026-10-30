-- Keyword Alerts: notifies you when a message contains one of your keywords.
--
--   /alerts add <word>     start watching for <word> (case-insensitive)
--   /alerts remove <word>  stop watching for <word>
--   /alerts list           show the watched keywords
--
-- Keywords are kept in the plugin's storage, so they survive restarts.

local keywords = ferrocord.storage.get("keywords") or {}

local function save()
    ferrocord.storage.set("keywords", keywords)
end

local function reply(text)
    ferrocord.log(text)
    ferrocord.notify("Keyword Alerts", text)
end

ferrocord.command("alerts", "Keyword alerts: add <word> | remove <word> | list", function(_channel_id, args)
    local sub, word = string.match(args, "^(%S*)%s*(.-)%s*$")
    sub = string.lower(sub or "")
    word = string.lower(word or "")

    if sub == "add" and word ~= "" then
        if not table.find(keywords, word) then
            table.insert(keywords, word)
            save()
        end
        reply("Watching for \"" .. word .. "\"")
    elseif sub == "remove" and word ~= "" then
        local index = table.find(keywords, word)
        if index then
            table.remove(keywords, index)
            save()
            reply("Stopped watching for \"" .. word .. "\"")
        else
            reply("\"" .. word .. "\" was not being watched")
        end
    elseif sub == "list" then
        if #keywords == 0 then
            reply("No keywords configured. Use /alerts add <word>.")
        else
            reply("Watching: " .. table.concat(keywords, ", "))
        end
    else
        reply("Usage: /alerts add <word> | /alerts remove <word> | /alerts list")
    end
end)

ferrocord.on("MESSAGE_CREATE", function(message)
    if #keywords == 0 or type(message.content) ~= "string" or message.content == "" then
        return
    end
    local me = ferrocord.state.me()
    if me and message.author and message.author.id == me.id then
        return
    end

    local text = string.lower(message.content)
    for _, word in ipairs(keywords) do
        if string.find(text, word, 1, true) then
            local channel = ferrocord.state.channel(message.channel_id)
            local where = channel and (channel.guild_id and ("#" .. channel.display_name) or channel.display_name) or "a channel"
            local author = message.author or {}
            local who = author.global_name or author.username or "Someone"
            ferrocord.notify("\"" .. word .. "\" in " .. where, who .. ": " .. message.content)
            return
        end
    end
end)
