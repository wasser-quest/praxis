{# Only the current turn is rendered; earlier turns stay raw in history. The
   clock lives here, not in the system prompt, so the system prompt stays
   byte-identical between turns and the model server can reuse its cache. #}
{% set current_input = user_prompt or user_message %}
{% if current_input %}
{{ current_input }}
{% else %}
No current user input was supplied to this render.
{% endif %}
{% if time %}

[Current time: {{ time }}{% if utc_now %} (UTC {{ utc_now }}){% endif %}]
{% endif %}
