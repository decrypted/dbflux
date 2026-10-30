# Trabajar con resultados

Los resultados se renderizan en pestañas de resultado dentro del documento. Cada
tabla o colección se abre en la vista que su driver declara para ese objeto, y
ofrece las demás vistas que declara:

- **Vista de tabla** para tablas relacionales y resultados de query.
- **Vista de árbol** para colecciones de documentos (por ejemplo MongoDB,
  DynamoDB), con vistas de **Tabla** y **JSON** de la misma página (ver
  [Colecciones de documentos](DOCUMENTS.md)).
- **Vista de tabla** con una vista de **Árbol** de las mismas filas para una
  tabla con columnas JSON (ver
  [Columnas JSON en una tabla](DOCUMENTS.md#columnas-json-en-una-tabla)).
- **Gráfico sobre la cuadrícula** para una serie temporal, como un measurement
  de InfluxDB o una hypertable de TimescaleDB, cuando sus filas se pueden
  graficar (ver
  [Tablas y colecciones de series temporales](CHARTS.md#tablas-y-colecciones-de-series-temporales)).
- **Vista clave-valor** para Redis (ver [Vista clave-valor](KEY_VALUE.md)).

Una tabla con una columna vectorial también lista las filas más
[similares](#filas-similares) a la seleccionada.

Los contenedores de tipo event-stream se abren como event streams cuando el
driver declara esa presentación.

## Resultados junto al editor

En una pestaña de consulta los resultados están debajo del editor. En una
pantalla ancha, muévelos a la derecha del editor con el botón de panel de la
cabecera de resultados, la entrada **Mostrar los resultados a la derecha** de
las acciones del panel o **Mover los resultados junto al editor o debajo** en la
paleta de comandos. El mismo control los vuelve a poner debajo. Arrastra el
divisor para cambiar el tamaño de cada lado. Las pestañas de consulta nuevas se
abren con la última posición elegida.

En cualquiera de las dos posiciones, `Ctrl+j` lleva el teclado del editor a los
resultados y `Ctrl+k` lo devuelve.

## Navegar el data grid

Cuando el panel de resultados tiene el foco:

- `j`/`k` (o `Down`/`Up`) — moverse entre filas.
- `h`/`l` (o `Left`/`Right`) — moverse entre columnas.
- `g`/`Shift+g` (o `Home`/`End`) — primera / última fila.
- `Ctrl+d`/`Ctrl+u` (o `PageDown`/`PageUp`) — recorrer filas por páginas.
- `[` / `]` — página anterior / siguiente de resultados (paginación).
- `f` enfoca la toolbar; `/` enfoca la búsqueda/filtro.
- `z` alterna el colapso del panel.
- `m` (o `Shift+F10`) abre el menú contextual de fila/celda.
- `=` añade los valores de las celdas seleccionadas al filtro `WHERE` de una
  tabla (ver [Filtrar por una celda](#filtrar-por-una-celda)).

El menú contextual de la celda (`m`, `Shift+F10` o clic derecho) está agrupado
para que quepa en pantalla. Arriba están **Copiar**, **Copiar como ▸** (CSV,
JSON, lista SQL IN, sentencias INSERT, tabla Markdown y, en una tabla, la fila
como query), **Ver valor** y **Añadir al filtro**. Después, la edición:
**Editar**, **Editar en el editor…**, **Editar como expresión SQL…**, **Pegar**
y **Establecer valor ▸** (NULL, DEFAULT y los identificadores que admite la
columna). Luego **Filtro ▸**, **Ordenar ▸**, **Columna ▸** (ocultar, mover,
fijar, ajustar ancho, Columnas…) y **Fila ▸** (inspeccionar, añadir, duplicar,
las acciones de fila de la conexión y, al final y aparte, Eliminar fila). Al
final, **Más ▸** (Generar SQL ▸, Graficar esta consulta, Buscar filas
similares) y **Barra de herramientas ▸**. Cada parte muestra solo lo que aplica
a la celda, y cada fila conserva su atajo. `Up` / `Down` recorren el menú,
`Enter` o `Right` ejecutan una fila o abren su submenú, y `Left` o `Escape`
vuelven atrás. Un menú abierto cerca del borde inferior o derecho de la ventana
se abre hacia arriba o se desplaza a la izquierda, los submenús se abren del
lado con espacio, y un menú más alto que la ventana se desplaza manteniendo
visible la fila resaltada; el menú de la cabecera de columna y el de exportar
caben en la ventana del mismo modo.

Con el ratón, un clic selecciona una celda y `Shift`+clic, o pulsar y arrastrar, selecciona el rango hasta otra celda. Un clic en un número de fila selecciona la fila entera, y `Shift`+clic en otro número de fila selecciona todas las filas intermedias. `Ctrl`+clic (`Cmd`+clic en macOS) en una celda o un número de fila la añade a la selección, o la quita cuando está seleccionada sola; al copiar varios rangos se copian las filas y columnas que abarcan, con las celdas fuera de la selección vacías. Los rangos añadidos sirven para copiar: borrar, duplicar, guardar y las demás acciones de fila actúan sobre la fila del cursor, un clic derecho selecciona la celda bajo el puntero y la vista de registro trata `Ctrl`+clic como un clic normal. Al arrastrar más allá de un borde de la cuadrícula, esta se desplaza en esa dirección, más deprisa cuanto más lejos esté el puntero, y el rango sigue creciendo hasta soltar el botón o volver con el puntero.

Cuando la selección abarca más de una celda, la barra de estado muestra cuántas celdas contiene y, para los números entre ellas, su suma, media, mínimo y máximo. Los NULL y el texto se cuentan pero quedan fuera de las cifras, y la barra de estado indica cuántos ha omitido. Los totales se calculan con las filas ya cargadas, incluidas las ediciones sin guardar, sin volver a consultar la base de datos.

## Vista de registro

Pulsa `i`, o usa el conmutador Registro en la barra de estado del
resultado, para mostrar la fila activa como una lista de Nombre / Valor que
ocupa toda el área de resultados. La cabecera indica la posición de la fila en
el resultado. Los campos se editan igual que las celdas de la cuadrícula, así
que los cambios sin guardar, Guardar fila y revertir funcionan igual en ambos
modos; `Up`/`Down` recorren los campos y `Left`/`Right` cambian de fila. Pulsa
`i` de nuevo para volver a la cuadrícula.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/results/record-view-dark.webp">
  <img src="../images/results/record-view-light.webp" alt="La vista de registro con una fila de cliente como lista de campos y valores">
</picture>

## Menú de la cabecera de columna

Haz clic derecho en la cabecera de una columna para abrir un menú limitado a
esa columna: ordenar ascendente o descendente, quitar el orden y todos los
operadores de filtro, en una sola lista plana. El clic izquierdo en la
cabecera selecciona la columna entera (`Shift`+clic selecciona todas las columnas hasta ella, `Ctrl`+clic, `Cmd`+clic en macOS, la añade o la quita); la flecha en el borde derecho de la cabecera alterna el orden, y muestra una flecha arriba/abajo tenue mientras la columna no está ordenada. Con **Hacer clic en la cabecera de una columna la ordena** activado (Settings > General > Data grid), un clic normal en la cabecera la ordena, como la flecha.

Para ordenar por varias columnas, haz `Shift`+clic en la flecha de otra columna:
pasa a ser la siguiente clave de orden, ascendente, y otro `Shift`+clic la vuelve
descendente y después la quita del orden. La flecha se queda con el clic, así
que `Shift`+clic en ella nunca amplía la selección de columnas como lo hace
`Shift`+clic en el resto de la cabecera. Con más de una clave, cada cabecera
ordenada muestra su posición (1, 2, 3) junto a la flecha, y las filas que
empatan en la primera clave se ordenan por la segunda, y así sucesivamente. Un
clic normal en cualquier flecha vuelve a ordenar solo por esa columna, en la
dirección que ya tenía si era clave. Una tabla cuya clave primaria tiene varias
columnas se abre ordenada por todas ellas, así que sus columnas clave muestran 1
y 2 desde el principio. El menú de la columna, y **Ordenar ▸** del menú de la
celda, ofrecen lo mismo con **Añadir al orden: columna ASC** y **Añadir
al orden: columna DESC**, que también cambian la dirección de una columna que ya
es clave, y **Quitar orden** elimina todas las claves. Una tabla envía todas las
claves a la base de datos en un único `ORDER BY`; un resultado de consulta se
ordena en memoria. En una tabla que sigue en su orden por defecto (su clave
primaria), la primera clave añadida sustituye ese orden, porque las filas
ordenadas primero por la clave primaria no dejan empates que otra columna pueda
ordenar; quitar el orden vuelve al orden por defecto.
`Shift+m` abre el mismo menú desde el teclado, para la columna de la celda
seleccionada. El menú termina con una sección **Columna** de ajustes de vista,
que cambian cómo la grilla muestra la columna y nunca la consulta. **Ajustar
ancho** ajusta la columna a su cabecera y a sus valores cargados, hasta 640
píxeles para que un valor JSON largo no saque el resto de la grilla de la
pantalla; un doble clic en el borde de redimensionado, a la derecha de la
cabecera, hace lo mismo.

**Ocultar columna** quita la columna de la pantalla, y **Columnas…** lista todas
las columnas con una marca en las visibles: `Enter`, `Space` o un clic muestra u
oculta la columna resaltada mientras la lista sigue abierta, **Mostrar todas las
columnas** devuelve todas las ocultas, y `Escape` la cierra. **Mover a la
izquierda** y **Mover a la derecha** mueven la columna un lugar; arrastrar una
cabecera sobre otra mueve su columna a ese lugar.

**Fijar columna** mantiene la columna en el borde izquierdo, justo después de
los números de fila, mientras la grilla se desplaza en horizontal, como un panel
inmovilizado: fija `id` y `created_at` para tenerlas a la vista en una tabla de
logs ancha. Las columnas fijadas van al principio de la grilla en el orden en
que se fijaron y muestran un alfiler en su cabecera; **Soltar columna** deja que
la columna vuelva a desplazarse, como la primera de las demás. Seleccionar,
editar, ordenar, redimensionar y los números de fila funcionan igual en las
columnas fijadas. Mover a la izquierda y Mover a la derecha se quedan en su lado
del borde de las fijadas; arrastrar una cabecera a través de él fija o suelta su
columna. Las columnas fijadas ocupan como mucho dos tercios del ancho de la
grilla, para que las demás siempre tengan sitio: en una grilla estrecha, las
fijadas que no caben se desplazan con las demás hasta que vuelva a haber ancho.

La consulta sigue leyendo todas las columnas (una tabla sigue ejecutando
`SELECT *`); para dejar una columna fuera de la consulta misma, usa la
proyección del constructor de consultas. Copiar, Copiar como CSV y Copiar como
JSON siguen la pantalla: las columnas visibles, en el orden en que la grilla las
muestra. Copiar fila, Copiar como consulta y Exportar usan la fila completa en
el orden propio del resultado, columnas ocultas incluidas. Una tabla recuerda su
disposición para su conexión, base de datos, esquema y tabla, al cerrarla y al
reiniciar DBFlux; volver a poner todas las columnas en el orden propio de la
tabla, todas visibles, la olvida. Un resultado de consulta conserva su
disposición mientras su pestaña está abierta. Una grilla de documentos mantiene
el orden de sus campos y solo ofrece Ajustar ancho.

## Panel de valor

Haz clic derecho en una celda y elige **Ver valor**, o pulsa `v`, para abrir la
celda en el panel inspector de la derecha. Muestra el valor como JSON, XML o
texto plano — detectado a partir del contenido, y solo cuando realmente se
analiza — con formato legible, compacto y ajuste de línea. Se puede editar allí:
**Guardar** confirma la fila directamente y **Revertir** descarta el cambio. El
panel sigue la celda seleccionada al moverte por la cuadrícula, salvo mientras
tenga un cambio sin guardar.

Una celda binaria (`bytea`, `BLOB`, `varbinary` y similares) se abre como volcado
hexadecimal, de solo lectura. Cuando sus bytes son una imagen PNG, JPEG, GIF,
WebP o BMP, reconocida por los propios bytes y no por el nombre de la columna,
el panel muestra la imagen, reducida para que quepa, con su tamaño en píxeles,
su formato y su tamaño en bytes debajo. **Hex** cambia al volcado hexadecimal e
**Imagen** vuelve a la imagen. **Guardar imagen como…** escribe los bytes
guardados en un archivo y **Copiar imagen** pone la imagen en el portapapeles;
ambas están también en la entrada Barra de herramientas del menú contextual
de la grilla (`m`). Una celda de texto con una imagen en base64, sola o como URI
`data:image/...;base64,`, se abre igual en la imagen, con **Texto** para volver
al texto.

Las imágenes de más de 32 MB o 32 megapíxeles no se decodifican, ni tampoco un
valor que termina antes que su imagen, como uno recortado por un límite de
tamaño: el panel indica el motivo encima del volcado hexadecimal. Las imágenes
animadas muestran su primer fotograma y SVG no se dibuja. El volcado
hexadecimal de un valor grande se detiene tras sus primeros 64 KB.

## Inspector de fila

Pulsa `Ctrl+Space`, o haz clic derecho en una fila y elige **Inspeccionar
fila**, para abrir la fila seleccionada en el panel inspector de la derecha.
Lista cada columna de la fila con su valor y marca las columnas de clave
primaria y foránea. El inspector sigue la fila seleccionada al moverte por la
cuadrícula; el botón de fijar de su cabecera lo mantiene en la fila actual.
**Editar**, **Duplicar** y **Eliminar**, al pie, actúan sobre la fila
inspeccionada cuando el resultado es editable. Pulsa `Ctrl+Space` de nuevo, o
el botón de cerrar, para ocultarlo.

Bajo **Referencias** el inspector lista las claves foráneas de la fila en ambas
direcciones:

- **Salientes** — una clave foránea de una sola columna de esta fila apunta a
  otra tabla. La entrada nombra esa tabla y la fila a la que apunta, por su
  valor legible una vez resuelto («María López · #8»), o por su clave hasta
  entonces. Haz clic en ella para abrir la tabla referenciada en esa fila:
  DBFlux reutiliza la pestaña existente de la tabla — abriendo una si no la
  hay — y reemplaza su filtro por una igualdad de clave primaria, de modo que
  la fila referenciada sea la visible.
- **Entrantes** — otras tablas tienen claves foráneas que apuntan a esta fila.
  Cada entrada nombra la tabla y cuenta cuántas de sus filas la referencian;
  los conteos se cargan en segundo plano. Estas entradas son solo
  informativas.

Las celdas de clave foránea de la cuadrícula pueden mostrar el mismo valor del
registro referenciado junto a su id: activa **Mostrar el valor del registro
referenciado en celdas de clave foránea** en [Configuración](SETTINGS.md). El
inspector de fila resuelve las referencias independientemente de ese ajuste.

## Filtrar resultados

La toolbar del data grid tiene un input de filtro `WHERE` que vuelve a ejecutar
la query con la condición que escribas. Para conexiones SQL soporta dos estilos:

- **`WHERE` en bruto** — escribe una condición plana (por ejemplo `status =
  'active'`). Este es el comportamiento por defecto.
- **Rutas relacionales (estilo ORM)** — escribe una ruta con puntos que recorre
  foreign keys, por ejemplo `created_by.email LIKE '%@acme.com'` o
  `created_by.organization.name = 'Acme'`. DBFlux resuelve la ruta contra los
  metadatos de foreign key de la tabla y hace los joins hasta la tabla
  referenciada por ti; no hace falta escribir los JOINs a mano.

Cuando un filtro relacional se resuelve, un chip muestra cuántos joins añadió.
Si un segmento es ambiguo o no se puede resolver, aparece un error inline con un
enlace **Open in builder** que abre el constructor visual de queries precargado
con los joins resueltos hasta ese punto. La entrada sin puntos siempre mantiene
el comportamiento de `WHERE` en bruto.

El input de filtro también ofrece autocompletado consciente del schema (misma
navegación que el builder — ver [Autocompletado consciente del
schema](QUERY_BUILDER.md#autocompletado-consciente-del-schema)).

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/results/filtering-dark.webp">
  <img src="../images/results/filtering-light.webp" alt="La tabla customers filtrada con una condición WHERE escrita en la barra de filtro">
</picture>

### Filtrar por una celda

El submenú **Filtro** del menú contextual de una celda (`m`, o clic derecho)
reduce el filtro al valor de la celda con `=`, `<>`, `<`, `>`, `LIKE`, `IS NULL`
y los demás operadores que admite el tipo de la columna, y su última entrada
limpia el filtro. Un clic central en una celda hace lo mismo con `=` en un solo
paso (`IS NULL` para una celda NULL) y selecciona la celda; con `Shift`, el clic
central excluye el valor (`<>`, o `IS NOT NULL` para una celda NULL).

`=`, o **Añadir al filtro** arriba del menú de la celda, añade las celdas
seleccionadas: una condición por columna seleccionada, con sus valores como
`columna = valor`, o como `columna IN (...)` cuando hay varias celdas de esa
columna, cada valor una vez. Todas las vías conservan el filtro actual como
término `AND`, entre paréntesis cuando tiene un `OR` de primer nivel, así que
`status = 500` pasa a `status = 500 AND "method" = 'getbets'`, y una condición
que el filtro ya contiene no se añade dos veces. Los valores binarios, JSON y
anidados no se pueden añadir así.

### Deshacer un filtro

Cada filtro `WHERE` nuevo con el que se lee una tabla es un paso de Deshacer y
Rehacer del grid, se aplique como se aplique (`Enter`, una entrada del
historial, un clic central o `=`, el submenú Filtro, limpiar el filtro). Con el foco en el
grid, Deshacer primero revierte las ediciones de celdas hechas desde la última
lectura de la tabla, de la más reciente a la más antigua, y después vuelve a
poner el filtro anterior y lee la tabla con él desde la primera página; Rehacer
avanza del mismo modo. Mientras el campo `WHERE` tiene el teclado, Deshacer y
Rehacer editan su texto como antes. Un paso de filtro nunca descarta ediciones
pendientes: con ediciones que Deshacer no puede revertir se rechaza con el mismo
mensaje que una recarga. También funciona en un grid de solo lectura, donde no
hay ediciones que deshacer. Los cambios de orden y de `LIMIT` no son pasos de
deshacer.

### Historial de filtros

Cada filtro con el que una tabla se leyó correctamente se guarda para esa tabla
(conexión, base de datos, schema y tabla), del más reciente al más antiguo,
hasta 50 por tabla, y se conserva al reiniciar. Aplicar de nuevo un filtro lo
sube al principio en lugar de listarlo dos veces, y un filtro que la base de
datos rechazó no se guarda. Solo se guarda el texto del filtro, nunca filas.

Abre la lista con el icono de historial a la izquierda del campo `WHERE`, con
**Filter history** en la entrada Toolbar del menú de la tabla (`m`), con
`Ctrl+r` (`Cmd+r` en macOS) en el campo, o con `Up` / `Down` en el campo
mientras está vacío o el cursor está al principio. La lista se abre con el
teclado en su campo de búsqueda, que empieza vacío. Escribir ahí reduce la lista
a los filtros que contienen todas las palabras escritas, en cualquier orden y
sin distinguir mayúsculas: `status 500` encuentra `status = 500 AND method =
'x'`. Las palabras encontradas se resaltan en las filas, y la coincidencia más
reciente queda resaltada para `Enter`. `Up` / `Down` mueven el resaltado,
`Ctrl+r` de nuevo pasa a la siguiente coincidencia más antigua y `Enter` aplica
el filtro resaltado. `Shift+Delete` o la × de una fila quita ese filtro, y la
última fila, **Clear history**, olvida todos los filtros de la tabla. `Escape`
primero vacía la búsqueda; un segundo `Escape` cierra la lista y vuelve al campo
`WHERE`.

El historial de queries de una colección de documentos (el botón de historial de
su barra de query) también se conserva al reiniciar.

### Comillas tras una comparación

Con **Insertar comillas tras una comparación en el filtro WHERE** activado
(Ajustes → General → Data grid, desactivado por defecto), escribir una
comparación tras una columna de texto o de fecha/hora de una tabla SQL inserta
las comillas de su valor: `method =` pasa a `method = '|'` con el cursor entre
ellas. Un espacio o una comilla escritos justo después, por costumbre, se
absorben; una vez escrito el valor, una `'` escrita delante de la comilla de
cierre pasa por encima en lugar de añadir otra; y `Backspace` justo después de la
inserción vuelve a quitar las comillas. Los números, booleanos, `IS NULL`,
`IN (...)`, el texto dentro de un literal y el texto pegado no reciben comillas.
Ver [Ajustes](SETTINGS.md) para los operadores.

## Filas similares

En una tabla con una columna vectorial (hoy, una tabla PostgreSQL con una
columna `vector` de pgvector), pulsa `Shift+s` o elige **Buscar filas
similares** en el menú de la fila para listar las filas más cercanas a la fila
seleccionada. La entrada del menú indica qué columna lee y cuántas filas lista,
o por qué no puede ejecutarse: la fila no tiene valor en esa columna, o la
cuadrícula tiene cambios sin guardar, que hay que guardar o revertir antes.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/results/similar-rows-dark.webp">
  <img src="../images/results/similar-rows-light.webp" alt="La tabla products tras Buscar filas similares: la fila seleccionada fijada arriba como ancla, sus diez filas más cercanas ordenadas por distancia L2 y la barra con la columna vectorial, la distancia y K">
</picture>

La cuadrícula muestra entonces la fila seleccionada fijada arriba, marcada como
`referencia`, y debajo sus 10 filas más cercanas, de la más cercana a la más
lejana. La primera columna contiene la distancia, bajo una cabecera que la
nombra. Una barra reemplaza la fila de filtro:

- El selector de columna elige la columna vectorial. Con el cursor sobre una
  columna vectorial, la búsqueda empieza en esa columna; si no, en la primera de
  la tabla.
- **L2**, **Coseno** y **Producto interno** cambian la distancia. Producto
  interno muestra el producto en sí, de mayor a menor.
- **K** fija cuántas filas listar, de 1 a 1000.
- **Volver a la tabla**, o `Esc`, regresa a la página de la tabla con su filtro.

La fila seleccionada queda fuera de sus vecinas por su clave primaria, las filas
sin vector no se listan y las filas similares son de solo lectura. Una distancia
que no es un número se muestra como un guion. Las columnas `halfvec` y
`sparsevec` no ofrecen la búsqueda.

## Editar y CRUD

En el data grid:

- `o` — añadir una fila.
- `x` — eliminar la fila seleccionada.
- `Delete` — marcar para eliminar cada fila que tenga una celda seleccionada.
- `r` — renombrar / editar (según el contexto).
- `y` — copiar la fila seleccionada.
- `Ctrl+c` (`Cmd+c`) — copiar la(s) celda(s) seleccionada(s) al portapapeles.

La barra de edición de una tabla editable empieza con **+** y **−**, como en un
IDE de bases de datos: **+** añade una fila debajo de la activa, como `o`, y
**−** marca para eliminar cada fila que tenga una celda seleccionada, como
`Delete`; Guardar las elimina, tras la confirmación de siempre. **−** está
desactivado mientras no hay ninguna celda seleccionada, **+** en una tabla que
no admite filas nuevas, y ambos en las filas similares; sus tooltips muestran
el atajo, o el motivo por el que están desactivados.

Con varias celdas seleccionadas, `Enter`, `F2` o un doble clic en una de ellas abre el editor para todas, y la barra de estado indica cuántas celdas cambiará la edición. El valor que confirmas se escribe en cada celda seleccionada del mismo tipo de columna que lo admita, como cambios sin guardar que Guardar y Revertir tratan igual que cualquier otra edición, y un solo deshacer (`u` o `Ctrl+z`) revierte la edición entera. Se omiten las celdas de columnas de solo lectura, las filas marcadas para eliminar y los valores que el editor en línea no edita (JSON, texto largo o de varias líneas), y también las columnas de clave cuando la selección abarca varias filas, porque un mismo valor en la clave de cada fila nunca podría guardarse; un valor elegido de la lista de valores permitidos de una columna solo va a columnas con la misma lista.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="../images/results/editing-dark.webp">
  <img src="../images/results/editing-light.webp" alt="La tabla customers con una celda editada aún sin guardar y los botones Save y Revert habilitados">
</picture>

### Editar valores largos

Las columnas JSON y los valores largos se abren en el editor modal de celdas.
Una celda de texto cuyo contenido se interpreta como un objeto o un array JSON,
o como XML, se abre ahí en el mismo editor que recibe una columna JSON,
detectado igual que en el panel de valor: con formato legible, resaltado de
sintaxis, números de línea y plegado, una línea de estado que indica si el
texto sigue siendo válido, y **Format** y **Compact**. El título nombra el tipo
de la columna y el formato, por ejemplo `Editar raw_result (TEXT · JSON)`. Los
números y cadenas sueltos siguen siendo texto sin formato.

Mostrar un valor con formato no cambia cómo se guarda. Un valor guardado en una
sola línea se vuelve a guardar compacto, quitando solo los espacios entre
elementos, de modo que el orden de las claves, números como `2312.64496440` y
cadenas como `"2725.8328350479"` quedan exactamente como estaban. Un valor
guardado en varias líneas se guarda tal como se editó, y **Save** sin cambios no
crea ninguna edición pendiente. Si el texto de una columna de texto deja de ser
válido, el primer **Save** muestra un aviso y el segundo lo guarda tal como se
escribió; una columna JSON lo rechaza.

**Copy** copia al portapapeles el texto del editor tal como se ve, con las
ediciones sin guardar, y **Copy compact** lo copia compactado. Arrastra el borde
derecho o inferior del editor, o su esquina inferior derecha, para cambiar su
tamaño, o maximízalo con el botón junto al botón de cerrar o con un doble clic en
su barra de título. El editor queda dentro del panel sobre el que se abre, así
que su barra de título y sus botones siguen a la vista con cualquier tamaño, por
largo que sea el valor. El tamaño se recuerda para el siguiente valor; **Reset
size**, el botón anterior al de maximizar, devuelve al editor su tamaño
predeterminado y olvida el recordado.

| Teclas | Acción |
|--------|--------|
| `Ctrl+s` (`Cmd+s`) | Guardar |
| `Ctrl+Shift+k` / `Ctrl+Shift+h` | Copiar / Copiar compacto (`Cmd` en macOS) |
| `Ctrl+Shift+u` / `Ctrl+Shift+j` | Formatear / Compactar |
| `Ctrl+Shift+Enter` | Maximizar o restaurar el editor |
| `Ctrl+Shift+Backspace` | Restablecer el tamaño predeterminado del editor |

`Ctrl+c` (`Cmd+c`) sigue copiando el texto seleccionado.
### Expresiones SQL en celdas

En una tabla de MySQL, MariaDB, PostgreSQL, SQLite o SQL Server, una celda puede tomar una expresión SQL en lugar de un valor, como `NOW()`, `"qty" + 1` o `DEFAULT`. La expresión se guarda sin comillas en el `UPDATE` o `INSERT` de la fila, y la base de datos calcula el valor.

- **Editar como expresión SQL…** en el menú contextual de la celda, o `Ctrl+Alt+E` (`Cmd+Alt+E` en macOS), abre el editor con su interruptor `fx` activado. En una celda de una selección mayor, la expresión va a cada celda seleccionada, como en una edición de varias celdas.
- Con el editor abierto, un clic en `fx` o el mismo atajo alterna entre valor y expresión, conservando lo escrito.
- Mientras `fx` está activado, una lista bajo el editor sugiere expresiones para el tipo de la columna y la base de datos, y `Abajo` / `Arriba` las escriben en el editor una a una. Se puede escribir cualquier otra expresión.

| Tipo de columna | MySQL / MariaDB | PostgreSQL | SQLite | SQL Server |
|---|---|---|---|---|
| Fecha y hora | `NOW()`, `CURRENT_TIMESTAMP`, `UTC_TIMESTAMP()`, `DATE_ADD(NOW(), INTERVAL 1 DAY)` | `now()`, `current_timestamp`, `now() + interval '1 day'` | `CURRENT_TIMESTAMP`, `datetime('now')`, `datetime('now', 'localtime')`, `datetime('now', '+1 day')` | `GETDATE()`, `CURRENT_TIMESTAMP`, `SYSDATETIME()`, `GETUTCDATE()`, `DATEADD(day, 1, GETDATE())` |
| Fecha | `CURDATE()`, `CURRENT_DATE`, `UTC_DATE()` | `current_date`, `current_date + 1` | `CURRENT_DATE`, `date('now')` | `CAST(GETDATE() AS date)` |
| Hora | `CURTIME()`, `CURRENT_TIME`, `UTC_TIME()` | `current_time`, `localtime` | `CURRENT_TIME`, `time('now')` | `CAST(GETDATE() AS time)` |
| uuid | `UUID()` (`char(36)`), `UNHEX(REPLACE(UUID(), '-', ''))` (`binary(16)`) | `gen_random_uuid()` | | `NEWID()` |
| Número | `col + 1`, `col - 1` | igual | igual | igual |
| Texto | `CONCAT(col, '')`, `UPPER`, `LOWER`, `TRIM` | `col \|\| ''`, `upper`, `lower`, `trim` | `col \|\| ''`, `upper`, `lower`, `trim` | `CONCAT(col, '')`, `UPPER`, `LOWER`, `LTRIM(RTRIM(col))` |
| Cualquiera | `NULL`, `DEFAULT` | `NULL`, `DEFAULT` | `NULL` | `NULL`, `DEFAULT` |

Una expresión pendiente se muestra en la celda como `fx NOW()` en cursiva hasta que se guarda; después de guardar, la fila muestra el valor que calculó la base de datos. Deshacer, rehacer y Revertir la tratan como cualquier otra edición, y las celdas de una fila nueva también la admiten. Una expresión con un comentario, un `;` o una `,` fuera de paréntesis se rechaza al confirmarla, porque podría cambiar más que la celda (un comentario podría ocultar el `WHERE` del `UPDATE`). Las demás bases de datos, los resultados de consultas y las colecciones de documentos no ofrecen expresiones.

### Revisar los cambios antes de guardar

**Revisar** en la barra de edición, junto a Guardar y Revertir, o `Ctrl+Alt+S` (`Cmd+Alt+S` en macOS), muestra las sentencias que Guardar ejecutará para los cambios sin guardar de la tabla, en el orden en que las ejecuta: eliminaciones, luego filas nuevas, luego filas editadas. Las construye el propio guardado y las genera el driver de la conexión, así que son lo que recibe la base de datos, con las expresiones SQL sin comillas. Los valores van en las sentencias como literales; los drivers de MySQL, MariaDB, PostgreSQL, SQLite y SQL Server no envían parámetros enlazados. Un cambio que no se puede guardar (una fila sin su clave) aparece como un comentario que explica por qué.

Desde la revisión, **Guardar** (o `Enter`) ejecuta esas sentencias, sin la confirmación aparte que piden las eliminaciones; **Copiar SQL** las copia como script; **Cancelar** (o `Escape`) cierra la revisión y conserva los cambios. Con **Revisar los cambios antes de guardar** activado en Configuración > General > Cuadrícula de datos, cada Guardar en la cuadrícula de una tabla abre primero la revisión.

### IDs generados

Una celda puede recibir un identificador nuevo que genera DBFlux y se escribe como un valor normal, así que funciona con cualquier base de datos: **UUID v4** (aleatorio), **UUID v7** (ordenado por tiempo), **KSUID** (27 caracteres, ordenable por segundo) y **ULID** (26 caracteres, ordenable por milisegundo).

- **Establecer valor ▸** del menú contextual de la celda tiene una entrada por cada tipo que la columna puede guardar. En una celda de una selección mayor, cada celda seleccionada recibe su propio valor nuevo, nunca una copia, y un solo deshacer los revierte todos.
- En una columna que parece un identificador (un tipo `uuid`, una clave, una columna de texto del tamaño de uno de los tipos o un nombre como `user_id`), la lista bajo el editor en línea ofrece primero los tipos, con los UUID arriba; `Abajo` / `Arriba` escriben un valor nuevo en el editor. Al confirmarlo sobre una selección de varias celdas, cada celda vuelve a recibir su propio valor.
- El diálogo del editor de celdas muestra un botón **Generar …** por tipo.

Una columna `uuid` o `uniqueidentifier` recibe los UUID en su forma con guiones. Una columna de texto recibe los tipos que caben en su longitud declarada: un `char(36)` los UUID, un `char(27)` un KSUID, un `varchar(30)` un KSUID o un ULID, y una columna de longitud desconocida los cuatro. Las columnas binarias como `binary(16)` de MySQL no ofrecen IDs generados, porque la cuadrícula escribe texto y necesitan los 16 bytes crudos; ahí se usa la expresión `UNHEX(REPLACE(UUID(), '-', ''))`.

### Cuándo los resultados son editables

Los browses de tabla planos son editables cuando la tabla tiene una primary key.
Los resultados producidos por el **constructor visual de queries** (modo SELECT)
también son editables, pero solo cuando están demostrablemente vinculados a una
única tabla: el resultado mapea 1:1 a una tabla subyacente y cada columna de
primary key de esa tabla está proyectada con su nombre original. Las ediciones y
eliminaciones construyen entonces su `WHERE` a partir de los valores de primary
key proyectados.

Los JOINs están permitidos: las columnas de la tabla origen son editables,
mientras que las columnas unidas son de solo lectura.

Un resultado del builder recae en **solo lectura** — con una pista en la toolbar
explicando por qué — cuando se cumple alguna de estas condiciones:

- La query agrega o usa `GROUP BY` / `HAVING`.
- La proyección es un wildcard a través de un JOIN.
- Falta una columna de primary key o está proyectada bajo un alias.
- Las keys de la tabla aún no se han cargado desde la caché del schema (el grid
  se actualiza a editable en cuanto llegan las keys).

El SQL de forma libre escrito en el editor sigue siendo de solo lectura; la
edición inline solo aplica a browses de tabla planos y a SELECTs generados por
el builder.

### Resultados agregados

Cuando un resultado proviene de una query agrupada (`GROUP BY`), las filas
muestran la salida agregada y la edición está deshabilitada — añadir fila,
eliminar fila, editar celda e inspeccionar fila no están disponibles, con
tooltips explicativos. La paginación cuenta las filas agrupadas (no las filas
subyacentes), así que el total de páginas es correcto. Las columnas de agregado
conservan el tipo de columna correcto, así que graficar sigue funcionando.

## Copiar como CSV o JSON

El submenú **Copiar como ▸** del menú contextual de un resultado de tabla,
debajo de **Copiar**, empieza con **CSV** y **JSON**. Copian la selección, sea un rango o
varios elegidos con `Ctrl`+clic, con el mismo exportador que **Copiar filas
cargadas al portapapeles**: el CSV empieza con una fila de cabecera con los
nombres de las columnas, y el JSON es un array con un objeto por fila, con las
columnas como claves y los números, booleanos y `null` como tales. Se incluyen
todas las filas y columnas que abarca la selección; una celda dentro de esa área
pero fuera de la selección queda vacía en CSV y `null` en JSON. Las copias
siguen lo que muestra la cuadrícula: una edición sin guardar se copia con su
valor nuevo y una fila añadida aún sin guardar, en su lugar. En JSON, las
columnas con el mismo nombre (`a.id` y `b.id` de un join) comparten una clave,
que toma el valor de la última. **Copiar** (`Ctrl+c`) copia texto separado por
tabulaciones, también con las ediciones y filas añadidas sin guardar, sin
cabecera salvo que **Incluir los nombres de columna al copiar** esté activado en
Ajustes → General → Cuadrícula de datos, que lo empieza con una línea con los
nombres de las columnas copiadas.
`Ctrl+Alt+c` (`Cmd+Alt+c` en macOS) ejecuta **Copiar como JSON** desde el
teclado; el menú lo muestra junto a la entrada.

Debajo, el submenú tiene tres extractores más:

- **Copiar como lista IN de SQL** copia los valores seleccionados de una
  columna como lista para un `WHERE ... IN`, por ejemplo `('a', 'O''Brien')` o
  `(1, 2, 3)`, con las comillas que usa el driver de la conexión (comillas ANSI
  en un resultado sin conexión). Un valor seleccionado varias veces aparece una
  sola vez y `NULL` se omite, porque `IN (NULL)` no coincide con nada; una
  selección que solo tiene NULL no copia nada y lo avisa. Con varias columnas
  seleccionadas usa la de más a la izquierda. El menú de una colección no la
  ofrece.
- **Copiar como sentencias INSERT**, en una tabla, copia un `INSERT` por cada
  fila seleccionada con las columnas seleccionadas, escrito por el mismo
  generador del driver que **Copiar como query**. Cada fila aporta sus propios
  valores para esas columnas, aunque una celda dentro del área no esté
  seleccionada, y una fila con un valor que el driver no puede escribir se
  omite. No se ofrece en filas agrupadas ni en un join del constructor de
  consultas, cuyas columnas no son todas de la tabla.
- **Copiar como tabla Markdown** copia las mismas filas y columnas que Copiar
  como CSV como una tabla Markdown con fila de cabecera; las celdas `NULL` y
  las no seleccionadas quedan vacías, los bytes se escriben en hexadecimal
  `\x` y los arrays y documentos como JSON, y `|`, `&`, `<` y los saltos de
  línea dentro de los valores se escapan.

Los tres copian lo que muestra la cuadrícula, como Copiar como CSV: una edición
sin guardar con su valor nuevo y una fila añadida aún sin guardar, en su lugar.

## Copiar como query

El submenú **Copiar como ▸** termina con la fila como query (INSERT, UPDATE o
DELETE, bajo un título que nombra el lenguaje de la conexión), que genera una
sentencia de mutación específica del driver (o un envelope, para drivers no SQL)
a partir de la fila seleccionada usando el generador de queries propio del
driver. El menú de la vista de documentos lo conserva como submenú propio.

## Exportar

Pulsa `Ctrl+e` (`Cmd+e`) en el panel de resultados, o ejecuta **Export results**
desde el command palette. Los formatos disponibles dependen de la forma del
resultado e incluyen:

- **CSV**
- **JSON (pretty)** y **JSON (compact)**
- **Text**
- **Binary** (para resultados con forma binaria)

La parte superior del menú elige qué se exporta, y los formatos de **Guardar
como archivo** y **Copiar al portapapeles** que hay debajo siguen esa elección:

- **Filas**: **Seleccionadas** escribe las filas que tienen una celda
  seleccionada tal como las muestra la cuadrícula, con las ediciones y filas
  añadidas sin guardar, como Copiar como CSV; **Cargadas** las filas que tiene la cuadrícula (en una tabla, la
  página actual) y **Todas** todas las filas de la tabla o la colección (ver más
  abajo; solo se ofrece donde la cuadrícula puede volver a leerla página a
  página). **Seleccionadas** está desactivada, con el motivo bajo la fila, hasta
  que se selecciona una celda.
- **Columnas**: **Visibles**, la opción por defecto, escribe las columnas
  mostradas en el orden en que las muestra la cuadrícula, así que las columnas
  ocultas se omiten y las movidas o fijadas conservan su lugar. **Todas**
  escribe todas las columnas en el orden del resultado, incluidas las ocultas.
  **Seleccionadas** escribe las columnas mostradas que tienen una celda
  seleccionada, en el orden de la pantalla. Filas seleccionadas con columnas
  seleccionadas escriben las celdas de un rango seleccionado con sus valores;
  con varios rangos de `Ctrl`+clic, todas las filas y columnas que abarcan.
- **Incluir fila de encabezado (nombres de columna)**, activada por defecto, y
  **Separador** (coma, punto y coma o tabulador) se aplican al CSV guardado o
  copiado desde este menú y a Exportar todas las filas en CSV. No cambian Copiar
  como CSV del menú contextual.

El título sobre las opciones indica qué escriben los formatos, por ejemplo
"Exportar 12 filas seleccionadas, 4 columnas", y el aviso tras guardar o copiar
indica las mismas filas y columnas con la ruta o el número de bytes. El menú se
abre en el primer formato, así que `Ctrl+e` `Enter` sigue guardando de
inmediato. `k` o `Arriba` sube a las opciones; `Enter`, `Derecha` o `l` en una
fila de opción pasa a su siguiente valor, saltando los que no se aplican, o
marca la casilla del encabezado, y un clic elige un valor directamente. Tus
elecciones se guardan para todas las cuadrículas, no por tabla, y entre
reinicios.

Con **Filas** en **Todas**, el menú ofrece **Exportar todas las filas a un
archivo**, en CSV o en JSON con un objeto por línea. Vuelve a leer la tabla o la
colección completa con el filtro, el orden y la selección de columnas actuales
de la cuadrícula, una página a la vez, escribe las columnas que elige la opción
**Columnas** y la escribe en el único archivo que elijas, de modo que los datos
nunca tienen que caber en memoria. Una tabla mostrada mediante el constructor de
consultas o un filtro relacional se lee con esa misma consulta, desde su
desplazamiento y hasta su límite cuando lo define. La exportación se ejecuta
como una tarea en el panel de tareas, que muestra su progreso y permite
cancelarla; el aviso indica el total de filas y la ruta.

Una colección de documentos exportada como JSON escribe cada documento
completo, un objeto por línea, con todos sus campos. Como CSV se escribe con
las columnas que la cuadrícula muestra para la primera página, incluidos los
objetos expandidos. Los documentos de una colección no tienen por qué
compartir sus campos, así que en un CSV un campo que aparece por primera vez en
una página posterior no tiene columna, porque el encabezado ya está escrito; en
ese caso la exportación nombra esos campos en una advertencia.

Un resultado de consulta ofrece solo las filas cargadas, porque exportarlas
todas implicaría volver a ejecutar la consulta. Lo mismo ocurre con una búsqueda
de vecinos más cercanos: devuelve un número fijo de las filas más próximas, y la
cuadrícula ya las tiene todas.
