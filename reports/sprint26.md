# Sprint 26 — Etapa 4: DOM JavaScript, identidade e medição

Escopo: Etapa 4 do roadmap. Etapas 1–3 preservadas; Shadow DOM, custom elements e
reescrita de layout não foram tocados.

## 1. Resumo

| Item | Antes | Depois |
|---|---|---|
| Identidade | `wrap(id)` criava um objeto novo a cada chamada: `document.body === document.body` era `false` | cache por `NodeId` com `WeakRef` + `FinalizationRegistry`; identidade estável enquanto o nó vive |
| Medição | nenhuma | `getBoundingClientRect`, `offsetWidth/Height/Left/Top`, `clientWidth/Height`, lidos da geometria do layout real |
| `dataset` | ausente | `Proxy` com conversão `data-user-id` ⇄ `userId`, leitura/escrita/remoção |
| `createTextNode` | ausente | cria nó de texto solto, anexável |
| `remove()` | ausente | desanexa e limpa `parent` (antes `detach` deixava o pai antigo) |
| `insertAdjacentHTML` | ausente — `example.com` morria em `s.js: not a function` | quatro posições, parser existente, invalidação da Etapa 3 |
| `matches`/`closest` | ausentes | `naro_css::parse_selector_list_text` + `Selector::matches`, sem parser novo |
| `querySelector` em elemento | ausente | `naro_css::query_selector_within` / `_all_within` |
| `getAttribute` | devolvia `undefined` para atributo ausente | `null`, como a API |
| `getElementById` | varria a arena, encontrando nós removidos | percorre a árvore, então nó removido não é encontrado |

`example.com` agora executa `s.js` inteiro, sem `JS ERROR`: `CONTENT_HEIGHT` foi de
315,80 para 776,60 e os seis parágrafos traduzidos aparecem.

## 2. Arquivos e crates

| Arquivo | Responsabilidade |
|---|---|
| `crates/naro-html/src/lib.rs` | `push_detached_text` (nó de texto sem pai), `attach_after`; `detach` passa a limpar `parent` |
| `crates/naro-css/src/lib.rs` | `query_selector_within` / `query_selector_all_within` (busca restrita a uma subárvore) |
| `crates/naro-js/src/lib.rs` | `ElementBox`; tabela `geometry` no `Dom`; hosts `__naro_box`, `__naro_child_nodes`, `__naro_create_text`, `__naro_matches`, `__naro_attrs`, `__naro_insert_adjacent`, `__naro_query_within`, `__naro_query_all_within`, `__naro_url`; cache de wrappers com `WeakRef`; `dataset`, `matches`, `closest`, `remove`, `insertAdjacentHTML`, `getBoundingClientRect`, `offset*`, `client*`, `childNodes`, `parentElement`, `nextSibling`, `previousSibling`, `nodeType`, `isConnected`, `append`/`prepend`; `location`, `navigator`; `start_measured` |
| `crates/naro-engine/src/lib.rs` | layout provisório antes dos scripts; `push_geometry` após carga, relayout e `flush`; hook de re-medição depois de `DOMContentLoaded` |
| `crates/naro-app/src/main.rs` | nada novo nesta etapa (checks da Etapa 3 reaproveitados) |
| `tests/compat/26-dom-api.html`, `manifest.txt` | página da etapa |
| `README.md` | APIs de DOM, identidade e medição, limitações |

## 3. Identidade

O `NodeId` é um índice num `Vec<Node>` que **nunca recicla** posições: um nó removido
mantém o índice, então um wrapper antigo não pode passar a descrever outro nó. O cache
é `Map<NodeId, WeakRef<object>>`; `FinalizationRegistry` apaga a chave morta. Quando
`WeakRef` não existe (build sem ela), cai para um cache forte — identidade idêntica,
memória proporcional aos nós já vistos.

Consequências verificadas: consultas repetidas devolvem o mesmo objeto; acesso pela
árvore (`parentElement.children[0]`) devolve o mesmo objeto; mutações preservam a
identidade; nó removido continua sendo ele mesmo, com `parentNode === null` e
`isConnected === false`, e `getElementById` deixa de encontrá-lo.

## 4. Medição

A engine converte `BlockBox` em `naro_js::ElementBox` (borda, caixa, quatro espessuras
de borda) e entrega ao realm com `set_geometry`. `naro-js` não depende de
`naro-layout`; a conversão fica num único ponto do `naro-engine`.

Momentos em que a geometria é publicada:

1. depois da carga (layout definitivo);
2. depois de cada `relayout()` (flush de layout, resize);
3. **antes dos scripts**, quando o documento tem `<script>`: um layout provisório é
   construído para que `DOMContentLoaded` e timers possam medir;
4. **depois de `DOMContentLoaded`**, antes dos primeiros timers, quando há timer
   pendente — um handler que muda `width` e agenda um `setTimeout` que mede recebe o
   valor novo.

Regras de leitura:

| API | Base |
|---|---|
| `getBoundingClientRect()` | border box em coordenadas de página: `x/y/left/top`, `right/bottom`, `width/height`, mais `toJSON()` |
| `offsetWidth/offsetHeight` | largura/altura do border box |
| `offsetLeft/offsetTop` | posição relativa ao **pai direto** |
| `clientWidth/clientHeight` | padding box: border box menos as bordas |

## 5. DOM e invalidação

Toda mutação nova usa o mesmo caminho da Etapa 3 (`__naro_mutation` →
`Invalidation` → `Engine::flush`), sem mecanismo paralelo:

- `createTextNode` não invalida sozinho (nó solto); anexá-lo invalida como `childList`;
- `remove()` emite `childList` com `removedNodes`;
- `insertAdjacentHTML` emite um `childList` por nó inserido, no pai correto
  (`beforebegin`/`afterend` → pai do alvo; `afterbegin`/`beforeend` → o próprio alvo);
- `dataset` escreve/remove atributo e emite `attributes` com o nome real (`data-user-id`);
- posição inválida em `insertAdjacentHTML` não altera a árvore nem invalida nada.

## 6. `example.com`

`s.js` faz, entre outras coisas: `B.children[0].insertAdjacentHTML("afterend", ...)`,
`[...B.querySelectorAll("p")]`, `navigator.languages.some(...)`, `B.prepend(p)` e
`B.insertAdjacentHTML("afterbegin", "<style>…</style><svg …>")`.

Falhas encontradas e corrigidas, em ordem:

| Falha | Causa | Correção |
|---|---|---|
| `s.js: not a function` | `insertAdjacentHTML` ausente | implementado |
| `not a function` no passo seguinte | `querySelectorAll` não existia em elementos | `__naro_query_all_within` + método no wrapper |
| `cannot read property 'languages'` | `navigator` ausente | stub com `language`/`languages`/`userAgent` |
| `B.prepend is not a function` | `ParentNode.prepend/append` ausentes | implementados (aceitam string e nó) |

Resultado atual: `STATUS: Idle`, **0 `JS ERROR`**, `CONTENT_HEIGHT: 776,60`, seis
parágrafos em ar/zh/fr/ru/es/en renderizados (`reports/p4-example.png`). Antes desta
etapa o mesmo endereço terminava com `s.js: not a function` e apenas o parágrafo
original.

## 7. Testes

`cargo test --workspace` → **146 testes, 0 falhas** (133 → +13, todos em `naro-engine`).

| Teste | Cobre |
|---|---|
| `repeated_queries_return_the_same_object` | `querySelector` ×2, `getElementById`, `parentNode` |
| `identity_survives_a_mutation` | classe + atributo e consulta de novo |
| `a_removed_node_keeps_its_identity_and_is_detached` | `remove()`, `parentNode === null`, `isConnected`, `getElementById` |
| `bounding_rect_matches_the_layout` | rect, `offset*`, `client*` contra padding/borda conhecidos |
| `measurements_follow_a_geometry_change` | `style.width` → `relayout=1` e caixa de 250px |
| `measurements_follow_node_insertion_and_removal` | `insertBefore` + `remove` movem `#a` |
| `dataset_reads_writes_and_deletes` | leitura, escrita, alteração externa, remoção |
| `create_text_node_and_relationships` | `nodeType`, `lastChild`, `childNodes` × `children` |
| `insert_adjacent_html_covers_all_four_positions` | quatro posições + ordem dos irmãos |
| `insert_adjacent_html_ignores_an_unknown_position` | posição inválida não insere |
| `matches_and_closest_use_the_css_engine` | combinador, atributo, ancestral, ausência, o próprio elemento |
| `element_scoped_queries_stay_inside_the_subtree` | `element.querySelector` não escapa da subárvore |
| `a_paint_only_change_keeps_the_measurements` | fundo não relayouta nem move a caixa |

Compatibilidade: `naro --compat tests/compat` → **25 pass, 0 falhas, 1 unsup (26)**
(era 24/0/1). `26-dom-api.html` cobre identidade, medição, `dataset`, `matches`/
`closest`, as quatro posições de `insertAdjacentHTML`, `createTextNode`, `remove` e
uma medição feita de um `setTimeout` depois de mudar a largura (`before=100 later=150`).

## 8. Baseline

| Página | Etapa 3 | Etapa 4 | JS ERROR |
|---|---|---|---|
| `naro://start` | 606,40 | 606,40 | 0 = 0 |
| `example.com` | 315,80 · `s.js` | **776,60** | **1 → 0** |
| `rust-lang.org` | 4338,38 | 4338,38 | 0 = 0 |
| `google.com` | 480,33 | 480,33 | 0 = 0 |
| MDN | 12039,00 | 12039,00 | 6 = 6 |
| `24-units.html`, `25-dynamic-style.html` | PASS | PASS | — |

Nenhuma regressão: nenhum texto perdido em nenhuma das cinco páginas (checagem por
substring em todos os runs), nenhum erro novo, nenhum `STATUS: Error`. O único erro
que **desapareceu** é o de `example.com`, que era o alvo explícito da etapa.
`--bench`: parse 0,018 ms, style 0,34 ms, layout 0,06 ms, paint 0,53 ms — mesma faixa
da Etapa 3.

## 9. Limitações

- `offsetLeft`/`offsetTop` medem contra o pai direto, não contra o ancestral
  posicionado mais próximo (`offsetParent` não existe).
- `getBoundingClientRect` devolve coordenadas de página sem subtrair `scrollY`, e não
  tem `width/height` de conteúdo rolável.
- `clientWidth`/`clientHeight` não descontam barras de rolagem (a engine não as
  desenha) e não arredondam para inteiro.
- Elemento sem caixa (`display: none`, `head` e filhos) mede zero — igual ao
  navegador, mas sem o caso de elemento ainda não anexado.
- `innerHTML` continua não executando `<script>` (mesma regra de antes); a inserção é
  feita por grafting, então os nós são copiados sem identidade prévia — que é o
  correto, já que são nós novos.
- `insertAdjacentHTML` aceita qualquer string e delega ao parser tolerante existente;
  não valida a posição contra a especificação além das quatro aceitas.
- `dataset` não implementa `DOMStringMap` com `Symbol.iterator` nem ordenação de
  chaves garantida.
- `getElementById`/`querySelector` percorrem a árvore a cada chamada (sem índice).
- `location` é somente leitura, derivado da URL carregada; `assign`/`replace`/`reload`
  são no-ops.
- Layout provisório roda uma vez a mais em páginas com `<script>`; é descartado e
  substituído pelo layout definitivo após a execução dos scripts.

## 10. Etapas anteriores

Etapa 1 (seletores/cascata): nenhuma alteração; `naro-css` só ganhou duas funções de
busca restrita que reutilizam `parse_selector_list_text` e `Selector::matches`.
Etapa 2 (unidades/funções): nenhuma alteração.
Etapa 3 (invalidação): `flush`, `Invalidation` e `MutationObserver` são os mesmos; a
única adição é a publicação de geometria após o relayout.
