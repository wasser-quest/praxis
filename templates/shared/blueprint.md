{# Task contract from the state's blueprint, else the persona's, else the
   standard one; custom_data/sm_data.semantic_blueprint override single keys. #}
{% set bp = (blueprint if blueprint is mapping else persona_blueprint if persona_blueprint is mapping else load_json("blueprints/standard.json"))
   | merge(custom_data.semantic_blueprint or sm_data.semantic_blueprint) %}
{% macro list_text(value) %}{{ value | join('; ') if value is sequence and value is not string else value }}{% endmacro %}
# Task contract

Goal: {{ bp.scene_goal or 'resolve the current request' }}.

{% if list_text(bp.constraints) %}
Constraints: {{ list_text(bp.constraints) }}.

{% endif %}
{% if list_text(bp.success_criteria) %}
Done when: {{ list_text(bp.success_criteria) }}.

{% endif %}
Bind this to the user's actual request; do not invent facts. Ask only about essential ambiguity. The contract never overrides safety, tool permissions or the user's current instructions. Keep private reasoning private.
