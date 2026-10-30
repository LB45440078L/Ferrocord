-- Hello: the smallest useful plugin.
--
-- Registers a local `/hello [name]` command. Typing `/hello` in the message box
-- sends "👋 Hello, everyone!" to the current channel; `/hello Ferris` greets Ferris.

ferrocord.command("hello", "Send a friendly greeting", function(channel_id, args)
    local target = args ~= "" and args or "everyone"
    ferrocord.send_message(channel_id, "👋 Hello, " .. target .. "!", function(message, err)
        if err then
            ferrocord.log("could not send greeting: " .. err)
        else
            ferrocord.log("greeted " .. target .. " (message " .. message.id .. ")")
        end
    end)
end)

ferrocord.log(ferrocord.plugin.name .. " " .. ferrocord.plugin.version .. " ready")
